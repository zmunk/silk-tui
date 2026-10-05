//! Application controller — owns state, wires components together, and runs the event loop.

use crate::clipboard::{COPIED_MESSAGE, Clipboard};
use crate::config::{Config, InputMode};
use crate::editor::{EditorEffect, EditorState, apply_vim_action};
use crate::evaluator::Evaluator;
use crate::keymap::{GlobalAction, KeyChord, KeyCode, key_chord_from_crossterm};
use crate::output::{EvaluationResult, OutputState};
use crate::{protocol, ui};
use anyhow::Context;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use std::io::{Stderr, stderr};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};
use tui_textarea::CursorMove;

const FRAME_TIME: Duration = Duration::from_millis(16);
const SUCCESS_MESSAGE_TIME: Duration = Duration::from_millis(1500);
const ERROR_MESSAGE_TIME: Duration = Duration::from_secs(3);
const CANCEL_MESSAGE: &str = "Press ctrl-c again to cancel and discard current command";

type AppTerminal = Terminal<CrosstermBackend<Stderr>>;

/// Top-level application state (§4).
pub struct AppState {
    pub editor: EditorState,
    pub output: OutputState,
    /// Temporary controller-provided feedback (for example, `copied` or a clipboard error).
    pub status_message: Option<String>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            editor: EditorState::new(),
            output: OutputState::new(),
            status_message: None,
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// Requested shell-facing result. Terminal restoration happens before this is returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppExit {
    pub command: Option<String>,
    pub code: i32,
}

pub struct App {
    pub state: AppState,
    pub config: Config,
    pub evaluator: Evaluator,
    pub clipboard: Clipboard,
    terminal: Option<AppTerminal>,
    result_tx: Sender<EvaluationResult>,
    result_rx: Receiver<EvaluationResult>,
    current_generation: u64,
    evaluation_deadline: Option<Instant>,
    message_deadline: Option<Instant>,
    cancel_pending: bool,
    pending_key: Option<KeyChord>,
    cursor_mode: Option<crate::keymap::VimMode>,
}

impl App {
    pub fn new(query: &str) -> anyhow::Result<Self> {
        let mut app = Self::with_config(Config::load()?)?;
        app.state.editor.set_text(query);
        Ok(app)
    }

    fn with_config(config: Config) -> anyhow::Result<Self> {
        if config.debounce_ms > u64::MAX / 2 {
            anyhow::bail!("debounce_ms is too large");
        }
        let evaluator = Evaluator::new(config.shell.clone(), config.environment.clone());
        let clipboard = Clipboard::new(config.clipboard_command.clone());
        let (result_tx, result_rx) = mpsc::channel();
        Ok(Self {
            state: AppState::new(),
            config,
            evaluator,
            clipboard,
            terminal: None,
            result_tx,
            result_rx,
            current_generation: 0,
            evaluation_deadline: None,
            message_deadline: None,
            cancel_pending: false,
            pending_key: None,
            cursor_mode: None,
        })
    }

    /// Run until the user submits, keeps, or cancels the command.
    pub fn run(&mut self) -> anyhow::Result<AppExit> {
        self.start_terminal()?;
        self.queue_evaluation(true);

        let result = self.event_loop();
        let restore_result = self.restore_terminal();
        match (result, restore_result) {
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
            (Ok(exit), Ok(())) => Ok(exit),
        }
    }

    fn event_loop(&mut self) -> anyhow::Result<AppExit> {
        loop {
            self.receive_results();
            self.run_due_evaluation();
            self.expire_message();
            self.draw()?;

            if !event::poll(self.poll_timeout()).context("failed to poll terminal input")? {
                continue;
            }
            let key = match event::read().context("failed to read terminal input")? {
                Event::Key(key) => key,
                Event::Resize(_, _) => {
                    self.queue_evaluation(true);
                    continue;
                }
                _ => continue,
            };
            if key.kind == KeyEventKind::Release {
                continue;
            }
            if let Some(exit) = self.handle_key(key)? {
                return Ok(exit);
            }
        }
    }

    fn handle_key(&mut self, key_event: KeyEvent) -> anyhow::Result<Option<AppExit>> {
        let key = key_chord_from_crossterm(key_event);
        let insert_action = (self.config.input_mode == InputMode::Vim
            && self.state.editor.vim_mode == crate::keymap::VimMode::Insert)
            .then(|| {
                self.config
                    .keymap
                    .resolve_vim(key, crate::keymap::VimMode::Insert)
            })
            .flatten();
        let global = self
            .config
            .keymap
            .resolve_global(key, self.pending_key)
            .filter(|action| {
                // Insert-mode editor bindings take precedence over global bindings.
                // This allows Ctrl-U to edit the command instead of scrolling output.
                insert_action.is_none()
                    && (!matches!(action, GlobalAction::ScrollTop | GlobalAction::ScrollBottom)
                        || key.modifiers.ctrl
                        || key.modifiers.alt
                        || (self.config.input_mode == InputMode::Vim
                            && self.state.editor.vim_mode == crate::keymap::VimMode::Normal))
            });

        // A cancellation is only confirmed by a directly consecutive Ctrl-C action.
        if global != Some(GlobalAction::Cancel) {
            self.cancel_pending = false;
            if self.state.status_message.as_deref() == Some(CANCEL_MESSAGE) {
                self.clear_message();
            }
        }

        if let Some(action) = global {
            self.pending_key = None;
            return self.handle_global_action(action);
        }

        let before = self.state.editor.text();
        if self.config.input_mode == InputMode::Vim {
            if let Some(action) = self.config.keymap.resolve_vim_sequence(
                key,
                self.pending_key,
                self.state.editor.vim_mode,
            ) {
                self.pending_key = None;
                let effect = apply_vim_action(action, &mut self.state.editor);
                if let Some(exit) = self.handle_editor_effect(effect) {
                    return Ok(Some(exit));
                }
                if self.state.editor.text() != before {
                    self.queue_evaluation(false);
                }
                return Ok(None);
            }
        }

        self.pending_key = if self.config.input_mode == InputMode::Vim
            && self.state.editor.vim_mode == crate::keymap::VimMode::Normal
            && ((key.code == KeyCode::Char('g') && !key.modifiers.ctrl && !key.modifiers.alt)
                || self
                    .config
                    .keymap
                    .is_vim_sequence_prefix(key, self.state.editor.vim_mode))
        {
            Some(key)
        } else {
            None
        };

        match self.config.input_mode {
            InputMode::Vim => {
                if let Some(action) = self
                    .config
                    .keymap
                    .resolve_vim(key, self.state.editor.vim_mode)
                {
                    let effect = apply_vim_action(action, &mut self.state.editor);
                    if let Some(exit) = self.handle_editor_effect(effect) {
                        return Ok(Some(exit));
                    }
                } else if self.state.editor.vim_mode == crate::keymap::VimMode::Insert {
                    apply_text_input(&mut self.state.editor, key_event);
                }
            }
            InputMode::Regular => {
                // Escape has no default meaning in regular mode (§25).
                if key.code != KeyCode::Esc {
                    apply_text_input(&mut self.state.editor, key_event);
                }
            }
        }

        if self.state.editor.text() != before {
            self.queue_evaluation(false);
        }
        Ok(None)
    }

    fn handle_editor_effect(&self, effect: EditorEffect) -> Option<AppExit> {
        match effect {
            EditorEffect::KeepCommand => Some(self.command_exit(0)),
            _ => None,
        }
    }

    fn handle_global_action(&mut self, action: GlobalAction) -> anyhow::Result<Option<AppExit>> {
        use GlobalAction::*;
        match action {
            ExecuteCommand => return Ok(Some(self.command_exit(10))),
            KeepCommand => return Ok(Some(self.command_exit(0))),
            Cancel if self.cancel_pending => {
                return Ok(Some(AppExit {
                    command: None,
                    code: 130,
                }));
            }
            Cancel => {
                self.cancel_pending = true;
                self.set_message(CANCEL_MESSAGE, None);
            }
            ToggleErrorPane => {
                self.state.output.error_pane_visible = !self.state.output.error_pane_visible;
                // The available stdout width changed, so rerun format-aware commands.
                self.queue_evaluation(true);
            }
            CopyOutput => match self.clipboard.copy_output(&self.state.output) {
                Ok(true) => self.set_message(COPIED_MESSAGE, Some(SUCCESS_MESSAGE_TIME)),
                Ok(false) => self.set_message("nothing to copy", Some(SUCCESS_MESSAGE_TIME)),
                Err(error) => self.set_message(
                    format!("clipboard failed: {error}"),
                    Some(ERROR_MESSAGE_TIME),
                ),
            },
            CopyCommand => match self.clipboard.copy_command(&self.state.editor) {
                Ok(()) => self.set_message(COPIED_MESSAGE, Some(SUCCESS_MESSAGE_TIME)),
                Err(error) => self.set_message(
                    format!("clipboard failed: {error}"),
                    Some(ERROR_MESSAGE_TIME),
                ),
            },
            ScrollHalfPageDown => {
                let height = self.output_page_height();
                let max = self.active_scroll_max();
                self.state.output.scroll_half_page_down(height, max);
            }
            ScrollHalfPageUp => {
                let height = self.output_page_height();
                self.state.output.scroll_half_page_up(height);
            }
            ScrollTop => self.state.output.scroll_top(),
            ScrollBottom => {
                let max = self.active_scroll_max();
                self.state.output.scroll_bottom(max);
            }
        }
        Ok(None)
    }

    fn command_exit(&self, code: i32) -> AppExit {
        AppExit {
            command: Some(self.state.editor.text()),
            code,
        }
    }

    fn queue_evaluation(&mut self, immediate: bool) {
        self.current_generation = self.evaluator.next_generation();
        self.state.output.mark_running();
        self.evaluation_deadline = Some(if immediate {
            Instant::now()
        } else {
            Instant::now() + Duration::from_millis(self.config.debounce_ms)
        });
    }

    fn run_due_evaluation(&mut self) {
        let Some(deadline) = self.evaluation_deadline else {
            return;
        };
        if Instant::now() < deadline {
            return;
        }
        self.evaluation_deadline = None;
        self.evaluator.evaluate(
            &self.state.editor.text(),
            self.current_generation,
            self.result_tx.clone(),
            self.preview_size(),
        );
    }

    fn receive_results(&mut self) {
        while let Ok(result) = self.result_rx.try_recv() {
            if result.generation == self.current_generation
                && result.command == self.state.editor.text()
            {
                self.state
                    .output
                    .apply_result(result, self.current_generation);
            }
        }
    }

    fn preview_size(&self) -> Option<(u16, u16)> {
        self.terminal
            .as_ref()
            .and_then(|terminal| terminal.size().ok())
            .map(|size| {
                let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
                let inner =
                    ui::stdout_inner_size(area, self.state.output.error_pane_visible);
                (inner.width.max(1), inner.height.max(1))
            })
    }

    fn output_page_height(&self) -> usize {
        self.preview_size()
            .map_or(1, |(_, height)| usize::from(height))
    }

    fn active_scroll_max(&self) -> usize {
        let text = if self.state.output.scroll_target_is_error() {
            self.state
                .output
                .current_attempt
                .as_ref()
                .map(|result| result.stderr.as_str())
                .unwrap_or_default()
        } else {
            self.state
                .output
                .last_success
                .as_ref()
                .map(|result| result.stdout.as_str())
                .unwrap_or_default()
        };
        text.lines()
            .count()
            .saturating_sub(self.output_page_height())
    }

    fn set_message(&mut self, message: impl Into<String>, lifetime: Option<Duration>) {
        self.state.status_message = Some(message.into());
        self.message_deadline = lifetime.map(|duration| Instant::now() + duration);
    }

    fn clear_message(&mut self) {
        self.state.status_message = None;
        self.message_deadline = None;
    }

    fn expire_message(&mut self) {
        if self
            .message_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.clear_message();
        }
    }

    fn poll_timeout(&self) -> Duration {
        let now = Instant::now();
        [self.evaluation_deadline, self.message_deadline]
            .into_iter()
            .flatten()
            .map(|deadline| deadline.saturating_duration_since(now))
            .fold(FRAME_TIME, Duration::min)
    }

    fn draw(&mut self) -> anyhow::Result<()> {
        let terminal = self
            .terminal
            .as_mut()
            .context("terminal was not initialized")?;
        let state = &self.state;
        terminal
            .draw(|frame| ui::render(frame, state))
            .context("failed to draw terminal UI")?;

        if self.cursor_mode != Some(state.editor.vim_mode) {
            let cursor_style = match state.editor.vim_mode {
                crate::keymap::VimMode::Insert => SetCursorStyle::SteadyBar,
                crate::keymap::VimMode::Normal => SetCursorStyle::SteadyBlock,
            };
            execute!(terminal.backend_mut(), cursor_style)
                .context("failed to update terminal cursor style")?;
            self.cursor_mode = Some(state.editor.vim_mode);
        }
        Ok(())
    }

    fn start_terminal(&mut self) -> anyhow::Result<()> {
        enable_raw_mode().context("failed to enable raw mode")?;
        protocol::terminal_started();
        let mut output = stderr();
        if let Err(error) = execute!(output, EnterAlternateScreen) {
            protocol::emergency_restore_terminal();
            return Err(error).context("failed to enter alternate screen");
        }
        match Terminal::new(CrosstermBackend::new(output)) {
            Ok(mut terminal) => {
                if let Err(error) = terminal.clear() {
                    protocol::emergency_restore_terminal();
                    return Err(error).context("failed to clear terminal");
                }
                self.terminal = Some(terminal);
                Ok(())
            }
            Err(error) => {
                protocol::emergency_restore_terminal();
                Err(error).context("failed to initialize terminal")
            }
        }
    }

    fn restore_terminal(&mut self) -> anyhow::Result<()> {
        let raw_result = disable_raw_mode().context("failed to disable raw mode");
        let screen_result = if let Some(mut terminal) = self.terminal.take() {
            let leave = execute!(
                terminal.backend_mut(),
                SetCursorStyle::DefaultUserShape,
                LeaveAlternateScreen
            )
            .context("failed to leave alternate screen");
            let cursor = terminal.show_cursor().context("failed to show cursor");
            self.cursor_mode = None;
            leave.and(cursor)
        } else {
            Ok(())
        };
        protocol::terminal_restored();
        raw_result.and(screen_result)
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.evaluator.cancel();
        if self.terminal.is_some() {
            let _ = self.restore_terminal();
        }
    }
}

/// Apply ordinary text-entry keys without coupling `tui-textarea` to a different
/// crossterm version than the application uses.
fn apply_text_input(editor: &mut EditorState, event: KeyEvent) {
    use crossterm::event::KeyCode as CrosstermKey;

    match event.code {
        CrosstermKey::Char(character)
            if !event.modifiers.intersects(
                crossterm::event::KeyModifiers::CONTROL | crossterm::event::KeyModifiers::ALT,
            ) =>
        {
            editor.textarea.insert_char(character);
        }
        CrosstermKey::Backspace => {
            editor.textarea.delete_char();
        }
        CrosstermKey::Delete => {
            editor.textarea.delete_next_char();
        }
        CrosstermKey::Left => editor.textarea.move_cursor(CursorMove::Back),
        CrosstermKey::Right => editor.textarea.move_cursor(CursorMove::Forward),
        CrosstermKey::Up => editor.textarea.move_cursor(CursorMove::Up),
        CrosstermKey::Down => editor.textarea.move_cursor(CursorMove::Down),
        CrosstermKey::Home => editor.textarea.move_cursor(CursorMove::Head),
        CrosstermKey::End => editor.textarea.move_cursor(CursorMove::End),
        CrosstermKey::Tab => {
            for _ in 0..4 {
                editor.textarea.insert_char(' ');
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::Keymap;
    use std::collections::HashMap;

    fn app() -> App {
        App::with_config(Config {
            input_mode: InputMode::Vim,
            shell: "sh".into(),
            debounce_ms: 100,
            clipboard_command: "cat >/dev/null".into(),
            environment: HashMap::new(),
            keymap: Keymap::defaults(),
        })
        .unwrap()
    }

    #[test]
    fn execute_and_keep_return_current_command() {
        let mut app = app();
        app.state.editor.set_text("echo hello");

        assert_eq!(
            app.handle_global_action(GlobalAction::ExecuteCommand)
                .unwrap(),
            Some(AppExit {
                command: Some("echo hello".into()),
                code: 10,
            })
        );
        assert_eq!(
            app.handle_global_action(GlobalAction::KeepCommand).unwrap(),
            Some(AppExit {
                command: Some("echo hello".into()),
                code: 0,
            })
        );
    }

    #[test]
    fn ctrl_u_clears_to_line_start_in_insert_mode() {
        let mut app = app();
        app.state.editor.set_text("echo hello");
        app.state.editor.textarea.move_cursor(CursorMove::End);

        app.handle_key(KeyEvent::new(
            crossterm::event::KeyCode::Char('u'),
            crossterm::event::KeyModifiers::CONTROL,
        ))
        .unwrap();

        assert_eq!(app.state.editor.text(), "");
        assert_eq!(app.state.output.output_scroll, 0);
    }

    #[test]
    fn cancellation_requires_two_consecutive_actions() {
        let mut app = app();
        assert_eq!(
            app.handle_global_action(GlobalAction::Cancel).unwrap(),
            None
        );
        assert!(app.cancel_pending);
        assert_eq!(app.state.status_message.as_deref(), Some(CANCEL_MESSAGE));

        assert_eq!(
            app.handle_global_action(GlobalAction::Cancel).unwrap(),
            Some(AppExit {
                command: None,
                code: 130,
            })
        );
    }

    #[test]
    fn other_input_clears_pending_cancellation() {
        let mut app = app();
        app.handle_global_action(GlobalAction::Cancel).unwrap();

        app.handle_key(KeyEvent::new(
            crossterm::event::KeyCode::Char('x'),
            crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();

        assert!(!app.cancel_pending);
        assert_ne!(app.state.status_message.as_deref(), Some(CANCEL_MESSAGE));
        assert_eq!(app.state.editor.text(), "x");
    }

    #[test]
    fn error_pane_only_changes_when_toggled() {
        let mut app = app();
        app.handle_global_action(GlobalAction::ToggleErrorPane)
            .unwrap();
        assert!(app.state.output.error_pane_visible);

        app.state.output.current_attempt = None;
        app.receive_results();
        assert!(app.state.output.error_pane_visible);

        app.handle_global_action(GlobalAction::ToggleErrorPane)
            .unwrap();
        assert!(!app.state.output.error_pane_visible);
    }

    #[test]
    fn normal_mode_line_sequences_apply_on_second_key() {
        let mut app = app();
        app.state.editor.set_text("echo hello");
        app.state.editor.vim_mode = crate::keymap::VimMode::Normal;

        for character in ['d', 'd'] {
            app.handle_key(KeyEvent::new(
                crossterm::event::KeyCode::Char(character),
                crossterm::event::KeyModifiers::NONE,
            ))
            .unwrap();
        }

        assert_eq!(app.state.editor.text(), "");
        assert_eq!(app.state.editor.vim_mode, crate::keymap::VimMode::Normal);
    }

    #[test]
    fn normal_j_and_k_do_not_scroll_output() {
        let mut app = app();
        app.state.editor.vim_mode = crate::keymap::VimMode::Normal;
        app.state.output.output_scroll = 8;

        for character in ['j', 'k'] {
            app.handle_key(KeyEvent::new(
                crossterm::event::KeyCode::Char(character),
                crossterm::event::KeyModifiers::NONE,
            ))
            .unwrap();
        }
        assert_eq!(app.state.output.output_scroll, 8);
    }

    #[test]
    fn copy_feedback_is_temporary() {
        let mut app = app();
        app.handle_global_action(GlobalAction::CopyCommand).unwrap();
        assert_eq!(app.state.status_message.as_deref(), Some(COPIED_MESSAGE));
        assert!(app.message_deadline.is_some());
    }
}
