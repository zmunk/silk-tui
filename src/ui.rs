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

/// Render the complete UI as a pure view of `AppState`.
pub fn render(frame: &mut Frame<'_>, state: &AppState) {
    let areas = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(2),
        ])
        .split(frame.area());

    render_output(frame, areas[0], state);
    render_editor(frame, areas[1], state);
    render_footer(frame, areas[2], state);
}

fn render_output(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    if state.output.error_pane_visible {
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(70), Constraint::Percentage(30)])
            .split(area);
        render_stdout(frame, panes[0], state);
        render_stderr(frame, panes[1], state);
    } else {
        render_stdout(frame, area, state);
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
        .map_or_else(|| " ERRORS ".to_owned(), |code| format!(" ERRORS · EXIT {code} "));
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
}
