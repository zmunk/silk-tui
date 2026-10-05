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
/// Minimum number of visible columns kept between the cursor and the right edge
/// of the editor pane, as long as the line has that much text left to show.
const LOOKAHEAD_COLUMNS: usize = 5;

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
/// terminal `area`, whether the stderr pane is visible, and how many rows the
/// editor needs. Shared by the renderer and by preview-size calculation so
/// both stay in lockstep.
///
/// `editor_line_count` is the number of logical lines in the command buffer
/// (always at least 1); the editor pane grows to fit them up to
/// `max_editor_lines`, then scrolls instead of growing further.
pub fn calculate_layout(
    area: Rect,
    error_pane_visible: bool,
    editor_line_count: usize,
    max_editor_lines: usize,
) -> UiLayout {
    let editor_rows = editor_visible_height(editor_line_count, max_editor_lines) as u16 + 2;
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(editor_rows),
            Constraint::Length(1),
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

/// Number of visible editor rows for a buffer with `line_count` lines, capped
/// at `max_lines` (both clamped to at least 1).
fn editor_visible_height(line_count: usize, max_lines: usize) -> usize {
    line_count.max(1).min(max_lines.max(1))
}

/// Compute the usable interior of the stdout pane, using the same
/// `Block::inner()` semantics as the `Borders::ALL` block rendered in
/// [`render_stdout`]. This is the exact size passed to the evaluator as
/// `COLUMNS` / `LINES` and as the PTY `ws_col` / `ws_row`.
pub fn stdout_inner_size(
    area: Rect,
    error_pane_visible: bool,
    editor_line_count: usize,
    max_editor_lines: usize,
) -> Rect {
    let layout = calculate_layout(area, error_pane_visible, editor_line_count, max_editor_lines);
    stdout_block().inner(layout.stdout)
}

fn stdout_block() -> Block<'static> {
    Block::default().borders(Borders::ALL)
}

/// Render the complete UI as a pure view of `AppState`.
pub fn render(frame: &mut Frame<'_>, state: &AppState, max_editor_lines: usize) {
    let editor_line_count = state.editor.textarea.lines().len();
    let layout = calculate_layout(
        frame.area(),
        state.output.error_pane_visible,
        editor_line_count,
        max_editor_lines,
    );

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
        .title(output_title(status, state.animation_started.elapsed().as_millis()))
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
    let inner_height = usize::from(area.height.saturating_sub(2)).max(1);
    let line_len = state
        .editor
        .textarea
        .lines()
        .get(cursor_row)
        .map(|line| line.chars().count())
        .unwrap_or(0);
    let horizontal_scroll = editor_horizontal_scroll(cursor_column, line_len, inner_width);
    let vertical_scroll = editor_vertical_scroll(
        cursor_row,
        state.editor.textarea.lines().len(),
        inner_height,
    );
    let editor = Paragraph::new(state.editor.text())
        .block(block)
        .scroll((to_u16(vertical_scroll), to_u16(horizontal_scroll)));
    frame.render_widget(editor, area);

    // Ratatui hides the terminal cursor unless the renderer explicitly positions it.
    let cursor_x = area
        .x
        .saturating_add(1)
        .saturating_add(to_u16(cursor_column.saturating_sub(horizontal_scroll)));
    let cursor_y = area
        .y
        .saturating_add(1)
        .saturating_add(to_u16(cursor_row.saturating_sub(vertical_scroll)));
    frame.set_cursor_position((cursor_x, cursor_y));
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, state: &AppState) {
    let status = state.output.status;
    let errors_hint = if current_stderr_nonempty(state) {
        " · Ctrl-E errors"
    } else {
        ""
    };

    let default_hints = if state.editor.vim_mode == VimMode::Insert {
        format!("Ctrl-Y copy output · Alt-Y copy command{errors_hint}")
    } else if status == EvaluationStatus::Stale {
        format!("Alt-U restore last success · Ctrl-Y copy output{errors_hint}")
    } else {
        format!("Ctrl-Y copy output · Alt-Y copy command{errors_hint}")
    };
    let hints = state.status_message.as_deref().unwrap_or(&default_hints);
    let hints_style = if state.status_message.is_some() {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let footer = Paragraph::new(Line::styled(hints, hints_style));
    frame.render_widget(footer, area);
}

fn current_stderr_nonempty(state: &AppState) -> bool {
    state
        .output
        .current_attempt
        .as_ref()
        .is_some_and(|result| !result.stderr.is_empty())
}

/// Compute the editor's horizontal scroll so that at least `LOOKAHEAD_COLUMNS`
/// of buffer stay visible past the cursor, as long as the line has that much text
/// left. Near the end of the line we only guarantee two blank cells past the
/// final character, rather than padding out to the full lookahead.
fn editor_horizontal_scroll(cursor_column: usize, line_len: usize, inner_width: usize) -> usize {
    let visible_target = (cursor_column + LOOKAHEAD_COLUMNS).min(line_len + 1);
    visible_target.saturating_sub(inner_width.saturating_sub(1))
}

fn editor_vertical_scroll(cursor_row: usize, line_count: usize, inner_height: usize) -> usize {
    let lookahead = usize::from(cursor_row + 1 < line_count && inner_height > 1);
    (cursor_row + lookahead).saturating_sub(inner_height.saturating_sub(1))
}

fn output_title(status: EvaluationStatus, elapsed_ms: u128) -> String {
    const SPINNER: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    if status == EvaluationStatus::Running {
        let spinner = SPINNER[((elapsed_ms / 80) % SPINNER.len() as u128) as usize];
        format!(" OUTPUT {spinner} ")
    } else {
        " OUTPUT ".to_owned()
    }
}

fn status_color(status: EvaluationStatus) -> Color {
    match status {
        EvaluationStatus::Current => Color::Green,
        EvaluationStatus::Empty | EvaluationStatus::Stale | EvaluationStatus::Running => {
            Color::DarkGray
        }
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
        assert_eq!(status_color(EvaluationStatus::Running), Color::DarkGray);
        assert_eq!(status_color(EvaluationStatus::Empty), Color::DarkGray);
        assert_eq!(status_color(EvaluationStatus::Stale), Color::DarkGray);
    }

    #[test]
    fn editor_scroll_keeps_lookahead_buffer_when_room_remains() {
        // Plenty of text after the cursor: scroll keeps a 5-column buffer, not less.
        assert_eq!(editor_horizontal_scroll(10, 100, 20), 0);
        assert_eq!(editor_horizontal_scroll(18, 100, 20), 4);
    }

    #[test]
    fn editor_scroll_only_guarantees_two_blanks_past_end_of_line() {
        // Cursor at end of a 12-char line: only two blank cells past the end
        // need to be visible, not a full 5-column lookahead.
        let line_len = 12;
        assert_eq!(
            editor_horizontal_scroll(line_len, line_len, 10),
            line_len + 1 - 9
        );
        // One before the end behaves the same way.
        assert_eq!(
            editor_horizontal_scroll(line_len - 1, line_len, 10),
            line_len + 1 - 9
        );
    }

    #[test]
    fn vertical_scroll_shows_a_line_below_the_cursor() {
        assert_eq!(editor_vertical_scroll(4, 10, 5), 1);
        assert_eq!(editor_vertical_scroll(8, 10, 5), 5);
        assert_eq!(editor_vertical_scroll(9, 10, 5), 5);
        assert_eq!(editor_vertical_scroll(0, 10, 5), 0);
        assert_eq!(editor_vertical_scroll(4, 10, 1), 4);
    }

    #[test]
    fn output_header_only_animates_while_running() {
        for status in [EvaluationStatus::Empty, EvaluationStatus::Current, EvaluationStatus::Stale] {
            assert_eq!(output_title(status, 0), " OUTPUT ");
            assert_eq!(output_title(status, 80), " OUTPUT ");
        }
        assert_ne!(output_title(EvaluationStatus::Running, 0), output_title(EvaluationStatus::Running, 80));
    }

    #[test]
    fn errors_hint_only_appears_for_nonempty_stderr() {
        let mut state = AppState::new();
        state.output.current_attempt = Some(result("", "failure"));
        assert!(current_stderr_nonempty(&state));

        state.output.error_pane_visible = true;
        assert!(current_stderr_nonempty(&state));

        state.output.current_attempt = Some(result("", ""));
        assert!(!current_stderr_nonempty(&state));
    }

    #[test]
    fn large_scroll_offsets_saturate_for_ratatui() {
        assert_eq!(to_u16(12), 12);
        assert_eq!(to_u16(usize::MAX), u16::MAX);
    }

    #[test]
    fn preview_size_matches_stdout_inner_rect_without_error_pane() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = calculate_layout(area, false, 1, 5);
        assert!(layout.stderr.is_none());

        let inner = stdout_block().inner(layout.stdout);
        let preview = stdout_inner_size(area, false, 1, 5);
        assert_eq!(preview, inner);
        assert_eq!((preview.width, preview.height), (118, 34));
    }

    #[test]
    fn preview_size_matches_stdout_inner_rect_with_error_pane_split() {
        let area = Rect::new(0, 0, 120, 40);
        let layout = calculate_layout(area, true, 1, 5);
        assert!(layout.stderr.is_some());

        let inner = stdout_block().inner(layout.stdout);
        let preview = stdout_inner_size(area, true, 1, 5);
        assert_eq!(preview, inner);
        assert_eq!(preview.width, layout.stdout.width - 2);
        assert_eq!(preview.height, layout.stdout.height - 2);
    }

    #[test]
    fn editor_pane_grows_with_lines_up_to_the_configured_cap() {
        let area = Rect::new(0, 0, 120, 40);

        let one_line = calculate_layout(area, false, 1, 5);
        assert_eq!(one_line.editor.height, 3);

        let three_lines = calculate_layout(area, false, 3, 5);
        assert_eq!(three_lines.editor.height, 5);

        let many_lines = calculate_layout(area, false, 50, 5);
        assert_eq!(many_lines.editor.height, 7); // capped at 5 content rows + borders

        // Growing the editor pane shrinks the output pane accordingly.
        assert!(many_lines.stdout.height < one_line.stdout.height);
    }
}
