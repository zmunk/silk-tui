//! State-only terminal rendering (§11–13).
//!
//! This module does not execute commands, mutate editor state, or access the clipboard.

use crate::app::AppState;
use crate::keymap::VimMode;
use crate::output::EvaluationStatus;
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};

const NO_OUTPUT: &str = "(no output)";

/// Pure layout geometry for the Silk UI: stdout pane, optional stderr pane,
/// the editor, and the footer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiLayout {
    pub stdout: Rect,
    pub stderr: Option<Rect>,
    pub editor: Rect,
    pub footer: Rect,
}

/// Compute the exact `Rect`s used by [`render`], as a pure function of the
/// terminal `area` and whether the stderr pane is visible. Shared by the
/// renderer and by preview-size calculation so both stay in lockstep.
pub fn calculate_layout(area: Rect, error_pane_visible: bool) -> UiLayout {
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(area);
    let output_area = rows[0];
    let editor = rows[1];
    let footer = rows[2];

    let (stdout, stderr) = if error_pane_visible {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(output_area);
        (panes[0], Some(panes[1]))
    } else {
        (output_area, None)
    };

    UiLayout {
        stdout,
        stderr,
        editor,
        footer,
    }
}

/// Compute the usable interior of the stdout pane, using the same
/// `Block::inner()` semantics as the `Borders::ALL` block rendered in
/// [`render_stdout`]. This is the exact size passed to the evaluator as
/// `COLUMNS` / `LINES` and as the PTY `ws_col` / `ws_row`.
pub fn stdout_inner_size(area: Rect, error_pane_visible: bool) -> Rect {
    let layout = calculate_layout(area, error_pane_visible);
    stdout_block().inner(layout.stdout)
}

fn stdout_block() -> Block<'static> {
    Block::default().borders(Borders::ALL)
}

/// Render the complete UI as a pure view of `AppState`.
pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    let layout = calculate_layout(frame.area(), state.output.error_pane_visible);

    render_output(frame, &layout, state);
    render_editor(frame, layout.editor, state);
    render_footer(frame, layout.footer, state);
}

fn render_output(frame: &mut Frame<'_>, layout: &UiLayout, state: &AppState) {
    render_stdout(frame, layout.stdout, state);
    if let Some(stderr) = layout.stderr {
        render_stderr(frame, stderr, state);
    }
}

fn render_stdout(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let status = state.output.status;
    let stdout = state
        .output
        .last_success
        .as_ref()
        .map(|result| result.stdout.as_str())
        .unwrap_or_default();
    let empty = stdout.is_empty();
    let text = if empty { NO_OUTPUT } else { stdout };
    let text_style = if empty {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default()
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" OUTPUT · {} ", status_label(status)))
        .border_style(Style::default().fg(status_color(status)));
    let paragraph = Paragraph::new(text)
        .style(text_style)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((to_u16(state.output.output_scroll), 0));
    frame.render_widget(paragraph, area);
}

fn render_stderr(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let stderr = state
        .output
        .current_attempt
        .as_ref()
        .map(|result| result.stderr.as_str())
        .unwrap_or_default();
    let title = state
        .output
        .current_attempt
        .as_ref()
        .and_then(|result| result.exit_code)
        .map_or_else(
            || " ERRORS ".to_owned(),
            |code| format!(" ERRORS · EXIT {code} "),
        );
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(Color::DarkGray));
    let paragraph = Paragraph::new(stderr)
        .block(block)
        .wrap(Wrap { trim: false })
        .scroll((to_u16(state.output.error_scroll), 0));
    frame.render_widget(paragraph, area);
}

fn render_editor(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let (mode, mode_color) = match state.editor.vim_mode {
        VimMode::Insert => ("INSERT", Color::Blue),
        VimMode::Normal => ("NORMAL", Color::Magenta),
    };
    let block = Block::default().borders(Borders::ALL).title(Span::styled(
        format!(" {mode} "),
        Style::default().fg(mode_color).add_modifier(Modifier::BOLD),
    ));
    let (cursor_row, cursor_column) = state.editor.textarea.cursor();
    let inner_width = usize::from(area.width.saturating_sub(2)).max(1);
    let horizontal_scroll = cursor_column.saturating_sub(inner_width.saturating_sub(1));
    let editor = Paragraph::new(state.editor.text())
        .block(block)
        .scroll((to_u16(cursor_row), to_u16(horizontal_scroll)));
    frame.render_widget(editor, area);

    // Ratatui hides the terminal cursor unless the renderer explicitly positions it.
    let cursor_x = area
        .x
        .saturating_add(1)
        .saturating_add(to_u16(cursor_column.saturating_sub(horizontal_scroll)));
    let cursor_y = area.y.saturating_add(1);
    frame.set_cursor_position((cursor_x, cursor_y));
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let status = state.output.status;
    let mode = match state.editor.vim_mode {
        VimMode::Insert => "INSERT",
        VimMode::Normal => "NORMAL",
    };

    let mut status_line = vec![
        Span::styled(mode, Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(
            status_label(status),
            Style::default().fg(status_color(status)),
        ),
    ];
    if hidden_stderr(state) {
        status_line.extend([
            Span::raw("  "),
            Span::styled("! stderr", Style::default().fg(Color::DarkGray)),
        ]);
    }

    let hints = state.status_message.as_deref().unwrap_or_else(|| {
        if state.editor.vim_mode == VimMode::Insert {
            "Enter execute · Esc normal · Ctrl-Y copy output · Alt-Y copy command · Ctrl-E errors"
        } else {
            "q keep · i insert · Ctrl-Y copy output · Alt-Y copy command · Ctrl-E errors"
        }
    });
    let hints_style = if state.status_message.is_some() {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let footer = Paragraph::new(vec![
        Line::from(status_line),
        Line::styled(hints, hints_style),
    ]);
    frame.render_widget(footer, area);
}

fn hidden_stderr(state: &AppState) -> bool {
    !state.output.error_pane_visible
        && state
            .output
            .current_attempt
            .as_ref()
            .is_some_and(|result| !result.stderr.is_empty())
}

fn status_label(status: EvaluationStatus) -> &'static str {
    match status {
        EvaluationStatus::Empty => "EMPTY",
        EvaluationStatus::Running => "RUNNING",
        EvaluationStatus::Current => "CURRENT",
        EvaluationStatus::Stale => "STALE",
    }
}

fn status_color(status: EvaluationStatus) -> Color {
    match status {
        EvaluationStatus::Current => Color::Green,
        EvaluationStatus::Running => Color::Yellow,
        EvaluationStatus::Empty | EvaluationStatus::Stale => Color::DarkGray,
    }
}

fn to_u16(value: usize) -> u16 {
    value.min(u16::MAX as usize) as u16
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::{EvaluationKind, EvaluationResult};

    fn result(stdout: &str, stderr: &str) -> EvaluationResult {
        EvaluationResult {
            generation: 1,
            command: "command".into(),
            stdout: stdout.into(),
            stderr: stderr.into(),
            exit_code: Some(1),
            kind: EvaluationKind::Failure,
        }
    }

    #[test]
    fn status_styles_follow_spec_and_never_use_red() {
        assert_eq!(status_color(EvaluationStatus::Current), Color::Green);
        assert_eq!(status_color(EvaluationStatus::Running), Color::Yellow);
        assert_eq!(status_color(EvaluationStatus::Empty), Color::DarkGray);
        assert_eq!(status_color(EvaluationStatus::Stale), Color::DarkGray);
    }

    #[test]
    fn stderr_indicator_only_appears_for_hidden_nonempty_stderr() {
        let mut state = AppState::new();
        state.output.current_attempt = Some(result("", "failure"));
        assert!(hidden_stderr(&state));

        state.output.error_pane_visible = true;
        assert!(!hidden_stderr(&state));

        state.output.error_pane_visible = false;
        state.output.current_attempt = Some(result("", ""));
        assert!(!hidden_stderr(&state));
    }

    #[test]
    fn large_scroll_offsets_saturate_for_ratatui() {
        assert_eq!(to_u16(12), 12);
        assert_eq!(to_u16(usize::MAX), u16::MAX);
    }

    #[test]
    fn preview_size_matches_stdout_inner_rect_without_error_pane() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = calculate_layout(area, false);
        assert!(layout.stderr.is_none());

        let inner = stdout_block().inner(layout.stdout);
        let preview = stdout_inner_size(area, false);
        assert_eq!(preview, inner);
        assert_eq!((preview.width, preview.height), (118, 33));
    }

    #[test]
    fn preview_size_matches_stdout_inner_rect_with_error_pane_split() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = calculate_layout(area, true);
        assert!(layout.stderr.is_some());

        let inner = stdout_block().inner(layout.stdout);
        let preview = stdout_inner_size(area, true);
        assert_eq!(preview, inner);
        assert_eq!(preview.width, layout.stdout.width - 2);
        assert_eq!(preview.height, layout.stdout.height - 2);
    }
}
