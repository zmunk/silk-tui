//! Application controller — owns state, wires components together, and runs the event loop.

use crate::clipboard::{COPIED_MESSAGE, Clipboard};
use crate::config::{Config, InputMode};
use crate::editor::{EditorEffect, EditorState, FindMotion, apply_vim_action};
use crate::evaluator::Evaluator;
use crate::keymap::{GlobalAction, KeyChord, KeyCode, VimAction, key_chord_from_crossterm};
use crate::output::{EvaluationResult, EvaluationStatus, OutputState};
use crate::terminal::{TtyBackend, clone_tty, open_tty, restore_output};
use crate::{protocol, ui};
use anyhow::Context;
use crossterm::cursor::SetCursorStyle;
use crossterm::event::{self, EnableBracketedPaste, Event, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, disable_raw_mode, enable_raw_mode};
use ratatui::Terminal;
use std::io::IsTerminal;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};
use tui_textarea::CursorMove;

const FRAME_TIME: Duration = Duration::from_millis(16);
const SUCCESS_MESSAGE_TIME: Duration = Duration::from_millis(1500);
const ERROR_MESSAGE_TIME: Duration = Duration::from_secs(3);
const CANCEL_MESSAGE: &str = "Press ctrl-c again to cancel and discard current command";
const RESTORED_MESSAGE: &str = "restored last successful command";
const NOTHING_TO_RESTORE_MESSAGE: &str = "nothing to restore";

/// Tracks in-progress Vim Normal-mode grammar that spans more than one keypress:
/// the `c` operator awaiting its motion, and the `f`/`F`/`t`/`T`/`r` family awaiting
/// their target character (bare, or as the tail of a `c` + motion combination).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NormalPending {
    None,
    /// `c` pressed, awaiting a motion key.
    Operator,
    /// `f`/`F`/`t`/`T` pressed, awaiting the target character. `change` is `true`
    /// when this follows `c` (e.g. `cf`, `ct`).
    Find {
        motion: FindMotion,
        change: bool,
    },
    /// `r` pressed, awaiting the replacement character.
    Replace,
}

type AppTerminal = Terminal<TtyBackend>;

/// Top-level application state (§4).
pub struct AppState {
    pub editor: EditorState,
    pub output: OutputState,
    /// Temporary controller-provided feedback (for example, `copied` or a clipboard error).
    pub status_message: Option<String>,
    pub animation_started: Instant,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            editor: EditorState::new(),
            output: OutputState::new(),
            status_message: None,
            animation_started: Instant::now(),
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
    normal_pending: NormalPending,
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
            normal_pending: NormalPending::None,
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
            match event::read().context("failed to read terminal input")? {
                Event::Key(key) => {
                    if key.kind == KeyEventKind::Release {
                        continue;
                    }
                    if let Some(exit) = self.handle_key(key)? {
                        return Ok(exit);
                    }
                }
                Event::Paste(text) => self.handle_paste(&text),
                Event::Resize(_, _) => self.queue_evaluation(true),
                _ => {}
            }
        }
    }

    /// Insert a terminal paste verbatim, preserving embedded newlines as real
    /// line breaks instead of letting them fall through as Enter keypresses.
    fn handle_paste(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let before = self.state.editor.text();
        crate::editor::insert_text(&mut self.state.editor, text);
        if self.state.editor.text() != before {
            self.queue_evaluation(false);
        }
    }

    fn handle_key(&mut self, key_event: KeyEvent) -> anyhow::Result<Option<AppExit>> {
        let key = key_chord_from_crossterm(key_event);

        if self.config.input_mode == InputMode::Vim
            && self.state.editor.vim_mode == crate::keymap::VimMode::Normal
        {
            if let Some(outcome) = self.handle_normal_grammar(key) {
                return Ok(outcome);
            }
        }

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

    /// Handle multi-key Vim Normal-mode grammar (`c` + motion, `f`/`F`/`t`/`T`, `r`)
    /// that doesn't fit the single-chord `Keymap` model. Returns `Some(outcome)` if
    /// `key` was consumed by this grammar (whether or not it changed anything), or
    /// `None` if the caller should continue with its regular key handling.
    fn handle_normal_grammar(&mut self, key: KeyChord) -> Option<Option<AppExit>> {
        let pending = std::mem::replace(&mut self.normal_pending, NormalPending::None);

        if pending != NormalPending::None {
            if key.code == KeyCode::Esc {
                return Some(None);
            }
            let before = self.state.editor.text();
            match pending {
                NormalPending::None => unreachable!(),
                NormalPending::Replace => {
                    if let KeyCode::Char(ch) = key.code {
                        if !key.modifiers.ctrl && !key.modifiers.alt {
                            crate::editor::replace_char(&mut self.state.editor, ch);
                        }
                    }
                }
                NormalPending::Find { motion, change } => {
                    if let KeyCode::Char(ch) = key.code {
                        if !key.modifiers.ctrl && !key.modifiers.alt {
                            if change {
                                crate::editor::change_over_find(&mut self.state.editor, motion, ch);
                            } else {
                                crate::editor::apply_find_motion(
                                    &mut self.state.editor,
                                    motion,
                                    ch,
                                );
                            }
                        }
                    }
                }
                NormalPending::Operator => {
                    if key.code == KeyCode::Char('c') && !key.modifiers.ctrl && !key.modifiers.alt {
                        apply_vim_action(VimAction::ChangeWholeLine, &mut self.state.editor);
                    } else if let Some(motion) = find_motion_for_key(key) {
                        self.normal_pending = NormalPending::Find {
                            motion,
                            change: true,
                        };
                    } else {
                        self.apply_change_motion_key(key);
                    }
                }
            }
            if self.state.editor.text() != before {
                self.queue_evaluation(false);
            }
            return Some(None);
        }

        if key.modifiers.ctrl || key.modifiers.alt {
            return None;
        }
        match key.code {
            KeyCode::Char('c') => self.normal_pending = NormalPending::Operator,
            KeyCode::Char('r') => self.normal_pending = NormalPending::Replace,
            KeyCode::Char('f') => {
                self.normal_pending = NormalPending::Find {
                    motion: FindMotion::ForwardTo,
                    change: false,
                }
            }
            KeyCode::Char('F') => {
                self.normal_pending = NormalPending::Find {
                    motion: FindMotion::BackwardTo,
                    change: false,
                }
            }
            KeyCode::Char('t') => {
                self.normal_pending = NormalPending::Find {
                    motion: FindMotion::ForwardTill,
                    change: false,
                }
            }
            KeyCode::Char('T') => {
                self.normal_pending = NormalPending::Find {
                    motion: FindMotion::BackwardTill,
                    change: false,
                }
            }
            _ => return None,
        }
        Some(None)
    }

    /// Apply the `c` operator paired with a plain (non-find) motion key:
    /// `cl`, `cw`, `cW`, `ce`, `c0`, `c$`, `cb`.
    fn apply_change_motion_key(&mut self, key: KeyChord) {
        use crate::editor as ed;
        if key.modifiers.ctrl || key.modifiers.alt {
            return;
        }
        match key.code {
            KeyCode::Char('l') => {
                apply_vim_action(VimAction::ChangeChar, &mut self.state.editor);
            }
            KeyCode::Char('w') => ed::change_word(&mut self.state.editor),
            KeyCode::Char('W') => ed::change_word_big(&mut self.state.editor),
            KeyCode::Char('e') => ed::change_to_word_end(&mut self.state.editor),
            KeyCode::Char('0') => ed::change_to_line_start(&mut self.state.editor),
            KeyCode::Char('$') => {
                apply_vim_action(VimAction::ChangeToLineEnd, &mut self.state.editor);
            }
            KeyCode::Char('b') => ed::change_word_backward(&mut self.state.editor),
            _ => {}
        }
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
            RestoreLastValidCommand => {
                if self.state.output.status == EvaluationStatus::Stale {
                    if let Some(command) = self
                        .state
                        .output
                        .last_success
                        .as_ref()
                        .map(|result| result.command.clone())
                    {
                        self.state.editor.set_text(&command);
                        self.queue_evaluation(true);
                        self.set_message(RESTORED_MESSAGE, Some(SUCCESS_MESSAGE_TIME));
                    }
                } else {
                    self.set_message(NOTHING_TO_RESTORE_MESSAGE, Some(SUCCESS_MESSAGE_TIME));
                }
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
        let editor_line_count = self.state.editor.textarea.lines().len();
        let max_editor_lines = self.config.max_editor_lines;
        self.terminal
            .as_ref()
            .and_then(|terminal| terminal.size().ok())
            .map(|size| {
                let area = ratatui::layout::Rect::new(0, 0, size.width, size.height);
                let inner = ui::stdout_inner_size(
                    area,
                    self.state.output.error_pane_visible,
                    editor_line_count,
                    max_editor_lines,
                );
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
        let max_editor_lines = self.config.max_editor_lines;
        terminal
            .draw(|frame| ui::render(frame, state, max_editor_lines))
            .context("failed to draw terminal UI")?;

        if self.cursor_mode != Some(state.editor.vim_mode) {
            let cursor_style = match state.editor.vim_mode {
                crate::keymap::VimMode::Insert => SetCursorStyle::SteadyBar,
                crate::keymap::VimMode::Normal => SetCursorStyle::SteadyBlock,
            };
            execute!(terminal.backend_mut().output(), cursor_style)
                .context("failed to update terminal cursor style")?;
            self.cursor_mode = Some(state.editor.vim_mode);
        }
        Ok(())
    }

    fn start_terminal(&mut self) -> anyhow::Result<()> {
        // Crossterm reads events/raw-mode state from stdin when it is a TTY.
        // Never replace stdin with the shell-protocol pipe.
        anyhow::ensure!(
            std::io::stdin().is_terminal(),
            "stdin must be connected to the terminal"
        );
        let mut output = open_tty().context("failed to open interactive /dev/tty")?;
        let rescue = clone_tty(&output).context("failed to clone interactive TTY")?;
        let rendering = clone_tty(&output).context("failed to clone TTY rendering handle")?;
        enable_raw_mode().context("failed to enable raw mode")?;
        protocol::terminal_started(rescue);
        if let Err(error) = execute!(output, EnterAlternateScreen, EnableBracketedPaste) {
            protocol::emergency_restore_terminal();
            return Err(error).context("failed to enter alternate screen");
        }
        match Terminal::new(TtyBackend::new(output, rendering)) {
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
            let leave = restore_output(terminal.backend_mut().output())
                .context("failed to restore terminal output");
            let cursor = terminal.show_cursor().context("failed to show cursor");
            self.cursor_mode = None;
            leave.and(cursor)
        } else {
            Ok(())
        };
        let result = raw_result.and(screen_result);
        if result.is_ok() {
            protocol::terminal_restored();
        } else {
            protocol::emergency_restore_terminal();
        }
        result
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

/// Map a bare key to the find/till motion it starts (`f`, `F`, `t`, `T`), if any.
fn find_motion_for_key(key: KeyChord) -> Option<FindMotion> {
    if key.modifiers.ctrl || key.modifiers.alt {
        return None;
    }
    match key.code {
        KeyCode::Char('f') => Some(FindMotion::ForwardTo),
        KeyCode::Char('F') => Some(FindMotion::BackwardTo),
        KeyCode::Char('t') => Some(FindMotion::ForwardTill),
        KeyCode::Char('T') => Some(FindMotion::BackwardTill),
        _ => None,
    }
}

/// Apply ordinary text-entry keys without coupling `tui-textarea` to a different
/// crossterm version than the application uses.
fn apply_text_input(editor: &mut EditorState, event: KeyEvent) {
    use crossterm::event::KeyCode as CrosstermKey;

    match event.code {
        // Enter means "execute" (handled as a global action); Ctrl-J is the
        // dedicated "insert a literal newline" key instead.
        CrosstermKey::Char('j')
            if event
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
                && !event
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::ALT) =>
        {
            editor.textarea.insert_newline();
        }
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
            max_editor_lines: 5,
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
    fn ctrl_q_quits_from_both_vim_modes() {
        let ctrl_q = KeyEvent::new(
            crossterm::event::KeyCode::Char('q'),
            crossterm::event::KeyModifiers::CONTROL,
        );

        let mut normal_app = app();
        normal_app.state.editor.set_text("echo hello");
        normal_app.state.editor.vim_mode = crate::keymap::VimMode::Normal;
        assert_eq!(
            normal_app.handle_key(ctrl_q).unwrap(),
            Some(AppExit {
                command: Some("echo hello".into()),
                code: 0,
            })
        );

        let mut insert_app = app();
        insert_app.state.editor.set_text("echo hello");
        insert_app.state.editor.vim_mode = crate::keymap::VimMode::Insert;
        assert_eq!(
            insert_app.handle_key(ctrl_q).unwrap(),
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
    fn normal_mode_new_editing_keys_are_wired() {
        for (text, keys, expected, mode) in [
            (
                "first\nsecond",
                "O",
                "\nfirst\nsecond",
                crate::keymap::VimMode::Insert,
            ),
            (
                "first\nsecond",
                "J",
                "first second",
                crate::keymap::VimMode::Normal,
            ),
            ("cat", "cl", "at", crate::keymap::VimMode::Insert),
            (
                "first\n\nthird",
                "jdd",
                "first\nthird",
                crate::keymap::VimMode::Normal,
            ),
        ] {
            let mut app = app();
            app.state.editor.set_text(text);
            app.state.editor.vim_mode = crate::keymap::VimMode::Normal;
            for character in keys.chars() {
                app.handle_key(KeyEvent::new(
                    crossterm::event::KeyCode::Char(character),
                    crossterm::event::KeyModifiers::NONE,
                ))
                .unwrap();
            }
            assert_eq!(app.state.editor.text(), expected);
            assert_eq!(app.state.editor.vim_mode, mode);
        }
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

    #[test]
    fn ctrl_j_inserts_a_literal_newline_instead_of_executing() {
        let mut app = app();
        app.state.editor.vim_mode = crate::keymap::VimMode::Insert;
        app.state.editor.set_text("echo one");
        app.state.editor.textarea.move_cursor(CursorMove::End);

        app.handle_key(KeyEvent::new(
            crossterm::event::KeyCode::Char('j'),
            crossterm::event::KeyModifiers::CONTROL,
        ))
        .unwrap();

        assert_eq!(app.state.editor.text(), "echo one\n");
    }

    #[test]
    fn pasted_newlines_insert_literal_lines_without_executing() {
        let mut app = app();
        app.state.editor.vim_mode = crate::keymap::VimMode::Insert;
        app.state.editor.set_text("");

        app.handle_paste("echo one\necho two");

        assert_eq!(app.state.editor.text(), "echo one\necho two");
    }

    #[test]
    fn restore_last_valid_command_only_applies_when_stale() {
        use crate::output::{EvaluationKind, EvaluationResult};

        let mut app = app();
        app.state.editor.set_text("echo broken");
        app.state.output.status = crate::output::EvaluationStatus::Current;
        app.handle_global_action(GlobalAction::RestoreLastValidCommand)
            .unwrap();
        assert_eq!(app.state.editor.text(), "echo broken");

        app.state.output.status = crate::output::EvaluationStatus::Stale;
        app.state.output.last_success = Some(EvaluationResult {
            generation: 1,
            command: "echo good".into(),
            stdout: "good".into(),
            stderr: String::new(),
            exit_code: Some(0),
            kind: EvaluationKind::Success,
        });
        app.handle_global_action(GlobalAction::RestoreLastValidCommand)
            .unwrap();
        assert_eq!(app.state.editor.text(), "echo good");
        assert_eq!(app.state.status_message.as_deref(), Some(RESTORED_MESSAGE));
    }

    #[test]
    fn vim_grammar_handles_change_find_and_replace() {
        let mut app = app();
        app.state.editor.vim_mode = crate::keymap::VimMode::Normal;
        app.state.editor.set_text("echo one two");

        // `r` replaces the character under the cursor.
        app.handle_key(KeyEvent::new(
            crossterm::event::KeyCode::Char('r'),
            crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();
        app.handle_key(KeyEvent::new(
            crossterm::event::KeyCode::Char('E'),
            crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();
        assert_eq!(app.state.editor.text(), "Echo one two");

        // `cw` changes the first word and enters Insert mode.
        app.handle_key(KeyEvent::new(
            crossterm::event::KeyCode::Char('c'),
            crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();
        app.handle_key(KeyEvent::new(
            crossterm::event::KeyCode::Char('w'),
            crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();
        assert_eq!(app.state.editor.text(), " one two");
        assert_eq!(app.state.editor.vim_mode, crate::keymap::VimMode::Insert);

        // `ct ` (change till space) from Normal mode on a fresh buffer.
        app.state.editor.vim_mode = crate::keymap::VimMode::Normal;
        app.state.editor.set_text("echo one two");
        app.handle_key(KeyEvent::new(
            crossterm::event::KeyCode::Char('c'),
            crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();
        app.handle_key(KeyEvent::new(
            crossterm::event::KeyCode::Char('t'),
            crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();
        app.handle_key(KeyEvent::new(
            crossterm::event::KeyCode::Char(' '),
            crossterm::event::KeyModifiers::NONE,
        ))
        .unwrap();
        assert_eq!(app.state.editor.text(), " one two");
        assert_eq!(app.state.editor.vim_mode, crate::keymap::VimMode::Insert);
    }
}
