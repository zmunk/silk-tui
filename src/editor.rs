//! Editor state and VimAction execution. Keymap → action resolution lives in keymap.rs;
//! action → editor behavior lives here.

use crate::keymap::{VimAction, VimMode};
use tui_textarea::{CursorMove, TextArea};

/// Effects that editor actions can produce, consumed by the app controller (§4 note).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorEffect {
    None,
    /// Return the command without executing (q).
    KeepCommand,
}

/// A snapshot of buffer contents + cursor position, for our own word-granularity
/// undo/redo stack (see below — we intentionally don't rely on `tui-textarea`'s
/// built-in undo, which records every primitive `insert_char`/`delete_next_char`
/// call as its own step).
type UndoSnapshot = (Vec<String>, (usize, usize));

/// Editor state: text buffer, cursor, Vim mode, undo/redo (§4, §16, §20).
pub struct EditorState {
    pub textarea: TextArea<'static>,
    pub vim_mode: VimMode,
    undo_stack: Vec<UndoSnapshot>,
    redo_stack: Vec<UndoSnapshot>,
}

/// Snapshots older than this are dropped, so the undo stack can't grow forever.
const MAX_UNDO_HISTORY: usize = 200;

impl EditorState {
    pub fn new() -> Self {
        Self {
            textarea: TextArea::default(),
            vim_mode: VimMode::Insert,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
        }
    }

    /// Set the initial command text (from --query).
    pub fn set_text(&mut self, text: &str) {
        // `split` preserves both an empty buffer and trailing newlines.
        let lines: Vec<String> = text.split('\n').map(String::from).collect();
        self.textarea = TextArea::from(lines);
        self.undo_stack.clear();
        self.redo_stack.clear();
    }

    /// Get the current command text.
    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
    }

    fn snapshot(&self) -> UndoSnapshot {
        (self.textarea.lines().to_vec(), self.textarea.cursor())
    }

    /// Record the buffer as it is *before* a mutating Vim command runs, so a
    /// later `u` can restore it in one step (e.g. all of `cw`'s delete + typed
    /// replacement, or a whole `dw`/`dd`), rather than undoing one character
    /// at a time.
    fn push_undo(&mut self) {
        self.undo_stack.push(self.snapshot());
        if self.undo_stack.len() > MAX_UNDO_HISTORY {
            self.undo_stack.remove(0);
        }
        self.redo_stack.clear();
    }

    fn restore(&mut self, lines: Vec<String>, cursor: (usize, usize)) {
        self.textarea = TextArea::from(lines);
        let (row, column) = cursor;
        for _ in 0..row {
            self.textarea.move_cursor(CursorMove::Down);
        }
        for _ in 0..column {
            self.textarea.move_cursor(CursorMove::Forward);
        }
    }

    /// Undo the most recent recorded command, one word/command at a time.
    pub fn undo(&mut self) {
        if let Some((lines, cursor)) = self.undo_stack.pop() {
            self.redo_stack.push(self.snapshot());
            self.restore(lines, cursor);
        }
    }

    /// Redo the most recently undone command.
    pub fn redo(&mut self) {
        if let Some((lines, cursor)) = self.redo_stack.pop() {
            self.undo_stack.push(self.snapshot());
            self.restore(lines, cursor);
        }
    }
}

impl Default for EditorState {
    fn default() -> Self {
        Self::new()
    }
}

/// Apply semantic Vim behavior independently of its configured key binding.
pub fn apply_vim_action(action: VimAction, editor: &mut EditorState) -> EditorEffect {
    use VimAction::*;

    match action {
        CursorLeft => editor.textarea.move_cursor(CursorMove::Back),
        CursorRight => editor.textarea.move_cursor(CursorMove::Forward),
        CursorUp => editor.textarea.move_cursor(CursorMove::Up),
        CursorDown => editor.textarea.move_cursor(CursorMove::Down),
        BeginningOfLine => editor.textarea.move_cursor(CursorMove::Head),
        EndOfLine => editor.textarea.move_cursor(CursorMove::End),
        WordForward => editor.textarea.move_cursor(CursorMove::WordForward),
        WordBackward => editor.textarea.move_cursor(CursorMove::WordBack),
        WordForwardBig => word_forward_big(editor),
        WordBackwardBig => word_backward_big(editor),
        WordEnd => move_to_word_end(editor),
        EnterInsertMode => {
            editor.push_undo();
            editor.vim_mode = VimMode::Insert;
        }
        ExitInsertMode => {
            // Match Vim: leaving Insert mode places the Normal-mode cursor on
            // the character immediately left of the insertion point.
            let (_, column) = editor.textarea.cursor();
            if column > 0 {
                editor.textarea.move_cursor(CursorMove::Back);
            }
            editor.vim_mode = VimMode::Normal;
        }
        AppendInsertMode => {
            editor.push_undo();
            editor.textarea.move_cursor(CursorMove::Forward);
            editor.vim_mode = VimMode::Insert;
        }
        InsertAtBeginning => {
            editor.push_undo();
            editor.textarea.move_cursor(CursorMove::Head);
            editor.vim_mode = VimMode::Insert;
        }
        AppendAtEnd => {
            editor.push_undo();
            editor.textarea.move_cursor(CursorMove::End);
            editor.vim_mode = VimMode::Insert;
        }
        OpenLineBelow => {
            editor.push_undo();
            editor.textarea.move_cursor(CursorMove::End);
            editor.textarea.insert_newline();
            editor.vim_mode = VimMode::Insert;
        }
        DeleteChar => {
            editor.push_undo();
            editor.textarea.delete_next_char();
        }
        DeleteToLineStart => {
            editor.push_undo();
            editor.textarea.delete_line_by_head();
        }
        ChangeWholeLine => {
            editor.push_undo();
            clear_current_line(editor);
            editor.vim_mode = VimMode::Insert;
        }
        ChangeToLineEnd => {
            editor.push_undo();
            editor.textarea.delete_line_by_end();
            editor.vim_mode = VimMode::Insert;
        }
        DeleteWholeLine => {
            editor.push_undo();
            delete_current_line_entirely(editor);
        }
        DeleteToLineEnd => {
            editor.push_undo();
            editor.textarea.delete_line_by_end();
        }
        DeleteWord => delete_word(editor),
        DeleteWordBig => delete_word_big(editor),
        Undo => editor.undo(),
        Redo => editor.redo(),
        KeepCommand => return EditorEffect::KeepCommand,
    }

    EditorEffect::None
}

/// Insert arbitrary text (e.g. a terminal paste) at the cursor, preserving any
/// embedded newlines as real line breaks rather than triggering Enter semantics.
pub fn insert_text(editor: &mut EditorState, text: &str) {
    for ch in text.chars() {
        match ch {
            '\n' => editor.textarea.insert_newline(),
            '\r' => {}
            ch => editor.textarea.insert_char(ch),
        }
    }
}

fn clear_current_line(editor: &mut EditorState) {
    editor.textarea.move_cursor(CursorMove::Head);
    editor.textarea.delete_line_by_end();
}

/// Vim's `dd` on a multi-line buffer must remove the line itself, not just its
/// content, so the following (or preceding, on the last line) line takes its place.
fn delete_current_line_entirely(editor: &mut EditorState) {
    clear_current_line(editor);
    let line_count = editor.textarea.lines().len();
    if line_count <= 1 {
        return;
    }
    let (row, _) = editor.textarea.cursor();
    if row + 1 < line_count {
        // Merge the now-empty current line with the one below it.
        editor.textarea.delete_next_char();
    } else {
        // Last line: merge upward into the previous line instead.
        editor.textarea.delete_char();
    }
}

/// Move the cursor to an absolute column on its current row, using repeated
/// single-column moves so it works uniformly across `tui-textarea` versions.
fn move_cursor_to_column(editor: &mut EditorState, target_col: usize) {
    let (_, column) = editor.textarea.cursor();
    if target_col > column {
        for _ in column..target_col {
            editor.textarea.move_cursor(CursorMove::Forward);
        }
    } else {
        for _ in target_col..column {
            editor.textarea.move_cursor(CursorMove::Back);
        }
    }
}

/// Vim's `W`: move to the start of the next WORD (whitespace-delimited, unlike `w`).
fn word_forward_big(editor: &mut EditorState) {
    let (row, column) = editor.textarea.cursor();
    let Some(line) = editor.textarea.lines().get(row) else {
        return;
    };
    let chars: Vec<char> = line.chars().collect();
    if chars.is_empty() {
        return;
    }

    let mut target = column;
    while target < chars.len() && !chars[target].is_whitespace() {
        target += 1;
    }
    while target < chars.len() && chars[target].is_whitespace() {
        target += 1;
    }
    let target = target.min(chars.len().saturating_sub(1));
    move_cursor_to_column(editor, target);
}

/// Vim's `B`: move to the start of the previous WORD (whitespace-delimited).
fn word_backward_big(editor: &mut EditorState) {
    let (row, column) = editor.textarea.cursor();
    let Some(line) = editor.textarea.lines().get(row) else {
        return;
    };
    let chars: Vec<char> = line.chars().collect();
    if column == 0 || chars.is_empty() {
        return;
    }

    let mut target = column - 1;
    while target > 0 && chars[target].is_whitespace() {
        target -= 1;
    }
    while target > 0 && !chars[target - 1].is_whitespace() {
        target -= 1;
    }
    move_cursor_to_column(editor, target);
}

/// A Vim character-seeking motion: `f`/`F` (to the char) or `t`/`T` (till it).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindMotion {
    ForwardTo,
    ForwardTill,
    BackwardTo,
    BackwardTill,
}

/// Compute the target column for a find/till motion on the current row, without
/// moving the cursor. Returns `None` if `target` does not occur in the required
/// direction.
fn find_target_column(editor: &EditorState, motion: FindMotion, target: char) -> Option<usize> {
    let (row, column) = editor.textarea.cursor();
    let line = editor.textarea.lines().get(row)?;
    let chars: Vec<char> = line.chars().collect();

    match motion {
        FindMotion::ForwardTo => (column + 1..chars.len()).find(|&i| chars[i] == target),
        FindMotion::ForwardTill => (column + 2..chars.len())
            .find(|&i| chars[i] == target)
            .map(|i| i - 1),
        FindMotion::BackwardTo => (0..column).rev().find(|&i| chars[i] == target),
        FindMotion::BackwardTill => {
            if column < 2 {
                return None;
            }
            (0..column - 1).rev().find(|&i| chars[i] == target).map(|i| i + 1)
        }
    }
}

/// Apply a standalone (non-operator) find/till motion. Returns `true` if `target`
/// was found and the cursor moved.
pub fn apply_find_motion(editor: &mut EditorState, motion: FindMotion, target: char) -> bool {
    match find_target_column(editor, motion, target) {
        Some(column) => {
            move_cursor_to_column(editor, column);
            true
        }
        None => false,
    }
}

/// Replace the character under the cursor with `ch` (Vim's `r`), leaving the
/// cursor in place and staying in Normal mode.
pub fn replace_char(editor: &mut EditorState, ch: char) {
    let (row, column) = editor.textarea.cursor();
    let Some(line) = editor.textarea.lines().get(row) else {
        return;
    };
    if column >= line.chars().count() {
        return;
    }
    editor.push_undo();
    editor.textarea.delete_next_char();
    editor.textarea.insert_char(ch);
    editor.textarea.move_cursor(CursorMove::Back);
}

/// Delete the half-open column range `[start_col, end_col)` on the current row,
/// recording one undo step for the whole range (the shared tail of every Vim
/// `d` operator + motion combination).
fn delete_range(editor: &mut EditorState, start_col: usize, end_col: usize) {
    editor.push_undo();
    move_cursor_to_column(editor, start_col);
    for _ in start_col..end_col {
        editor.textarea.delete_next_char();
    }
}

/// Delete the half-open column range `[start_col, end_col)` on the current row
/// and enter Insert mode at `start_col` (the shared tail of every Vim `c`
/// operator + motion combination).
fn change_range(editor: &mut EditorState, start_col: usize, end_col: usize) {
    editor.push_undo();
    move_cursor_to_column(editor, start_col);
    for _ in start_col..end_col {
        editor.textarea.delete_next_char();
    }
    editor.vim_mode = VimMode::Insert;
}

/// Vim's `dw`: delete from the cursor up to (not including) the start of the
/// next word, without crossing to the next line.
fn delete_word(editor: &mut EditorState) {
    let (row, start_col) = editor.textarea.cursor();
    let line_len = editor
        .textarea
        .lines()
        .get(row)
        .map(|line| line.chars().count())
        .unwrap_or(0);
    editor.textarea.move_cursor(CursorMove::WordForward);
    let (new_row, target_col) = editor.textarea.cursor();
    let end_col = if new_row != row { line_len } else { target_col };
    delete_range(editor, start_col.min(end_col), start_col.max(end_col));
}

/// Vim's `dW`: as [`delete_word`], but WORD-delimited (whitespace only).
fn delete_word_big(editor: &mut EditorState) {
    let (_, start_col) = editor.textarea.cursor();
    word_forward_big(editor);
    let (_, end_col) = editor.textarea.cursor();
    delete_range(editor, start_col.min(end_col), start_col.max(end_col));
}

fn word_end_target_column(chars: &[char], column: usize) -> usize {
    if column >= chars.len() {
        return column;
    }
    let mut target = column;
    if chars[target].is_whitespace() {
        while target < chars.len() && chars[target].is_whitespace() {
            target += 1;
        }
    } else if target + 1 < chars.len() {
        target += 1;
    }
    while target + 1 < chars.len() && !chars[target + 1].is_whitespace() {
        target += 1;
    }
    target
}

/// Vim's `cw`: behaves like `ce` when the cursor is on a non-blank character
/// (it does not consume trailing whitespace before the next word), and like a
/// plain word-delete when the cursor is on whitespace.
pub fn change_word(editor: &mut EditorState) {
    let (row, column) = editor.textarea.cursor();
    let Some(line) = editor.textarea.lines().get(row) else {
        return;
    };
    let chars: Vec<char> = line.chars().collect();
    if column >= chars.len() {
        editor.vim_mode = VimMode::Insert;
        return;
    }

    let end = if chars[column].is_whitespace() {
        let mut target = column;
        while target < chars.len() && chars[target].is_whitespace() {
            target += 1;
        }
        target
    } else {
        word_end_target_column(&chars, column) + 1
    };
    change_range(editor, column, end);
}

/// Vim's `cW`: as [`change_word`], but WORD-delimited (whitespace only).
pub fn change_word_big(editor: &mut EditorState) {
    let (row, column) = editor.textarea.cursor();
    let Some(line) = editor.textarea.lines().get(row) else {
        return;
    };
    let chars: Vec<char> = line.chars().collect();
    if column >= chars.len() {
        editor.vim_mode = VimMode::Insert;
        return;
    }

    let mut end = column;
    if chars[end].is_whitespace() {
        while end < chars.len() && chars[end].is_whitespace() {
            end += 1;
        }
    } else {
        while end < chars.len() && !chars[end].is_whitespace() {
            end += 1;
        }
    }
    change_range(editor, column, end);
}

/// Vim's `ce`: change to the end of the current/next word (inclusive).
pub fn change_to_word_end(editor: &mut EditorState) {
    let (row, column) = editor.textarea.cursor();
    let Some(line) = editor.textarea.lines().get(row) else {
        return;
    };
    let chars: Vec<char> = line.chars().collect();
    let end = word_end_target_column(&chars, column) + 1;
    change_range(editor, column, end);
}

/// Vim's `c0`: change from the start of the line up to (not including) the cursor.
pub fn change_to_line_start(editor: &mut EditorState) {
    let (_, column) = editor.textarea.cursor();
    change_range(editor, 0, column);
}

/// Vim's `cb`: change the previous word, from its start up to (not including) the cursor.
pub fn change_word_backward(editor: &mut EditorState) {
    let (_, column) = editor.textarea.cursor();
    word_backward_big_dry_run(editor); // positions cursor at the word start for reuse below
    let (_, start) = editor.textarea.cursor();
    change_range(editor, start, column);
}

fn word_backward_big_dry_run(editor: &mut EditorState) {
    // `cb` uses the small-word backward motion, not the WORD one.
    editor.textarea.move_cursor(CursorMove::WordBack);
}

/// Apply a Vim `c` operator paired with a find/till motion (`cf`, `cF`, `ct`, `cT`).
pub fn change_over_find(editor: &mut EditorState, motion: FindMotion, target: char) -> bool {
    let (_, column) = editor.textarea.cursor();
    let Some(found) = find_target_column(editor, motion, target) else {
        return false;
    };
    match motion {
        FindMotion::ForwardTo | FindMotion::ForwardTill => change_range(editor, column, found + 1),
        FindMotion::BackwardTo | FindMotion::BackwardTill => change_range(editor, found, column),
    }
    true
}

/// Vim's `e`: move to the final character of the current or next word.
fn move_to_word_end(editor: &mut EditorState) {
    let (row, column) = editor.textarea.cursor();
    let Some(line) = editor.textarea.lines().get(row) else {
        return;
    };
    let chars: Vec<char> = line.chars().collect();
    if column >= chars.len() {
        return;
    }

    let mut target = column;
    if chars[target].is_whitespace() {
        while target < chars.len() && chars[target].is_whitespace() {
            target += 1;
        }
    } else if target + 1 < chars.len() {
        target += 1;
    }
    while target + 1 < chars.len() && !chars[target + 1].is_whitespace() {
        target += 1;
    }

    for _ in column..target {
        editor.textarea.move_cursor(CursorMove::Forward);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> EditorState {
        let mut editor = EditorState::new();
        editor.set_text(text);
        editor
    }

    #[test]
    fn escape_and_i_switch_modes() {
        let mut editor = editor("text");
        editor.textarea.move_cursor(CursorMove::End);
        assert_eq!(editor.textarea.cursor(), (0, 4));

        apply_vim_action(VimAction::ExitInsertMode, &mut editor);
        assert_eq!(editor.vim_mode, VimMode::Normal);
        assert_eq!(editor.textarea.cursor(), (0, 3));

        apply_vim_action(VimAction::EnterInsertMode, &mut editor);
        assert_eq!(editor.vim_mode, VimMode::Insert);
    }

    #[test]
    fn escape_at_line_start_does_not_move_to_previous_line() {
        let mut editor = editor("first\nsecond");
        assert_eq!(editor.textarea.cursor(), (0, 0));

        apply_vim_action(VimAction::ExitInsertMode, &mut editor);

        assert_eq!(editor.textarea.cursor(), (0, 0));
        assert_eq!(editor.vim_mode, VimMode::Normal);
    }

    #[test]
    fn insert_transition_positions_are_correct() {
        let mut append = editor("abcd");
        append.vim_mode = VimMode::Normal;
        apply_vim_action(VimAction::AppendInsertMode, &mut append);
        assert_eq!(append.textarea.cursor(), (0, 1));
        assert_eq!(append.vim_mode, VimMode::Insert);

        let mut beginning = editor("abcd");
        beginning.textarea.move_cursor(CursorMove::End);
        apply_vim_action(VimAction::InsertAtBeginning, &mut beginning);
        assert_eq!(beginning.textarea.cursor(), (0, 0));
        assert_eq!(beginning.vim_mode, VimMode::Insert);

        let mut end = editor("abcd");
        apply_vim_action(VimAction::AppendAtEnd, &mut end);
        assert_eq!(end.textarea.cursor(), (0, 4));
        assert_eq!(end.vim_mode, VimMode::Insert);
    }

    #[test]
    fn horizontal_and_word_motions_are_semantic_actions() {
        let mut editor = editor("one two three");
        apply_vim_action(VimAction::CursorRight, &mut editor);
        assert_eq!(editor.textarea.cursor(), (0, 1));
        apply_vim_action(VimAction::CursorLeft, &mut editor);
        assert_eq!(editor.textarea.cursor(), (0, 0));

        apply_vim_action(VimAction::WordForward, &mut editor);
        assert_eq!(editor.textarea.cursor(), (0, 4));
        apply_vim_action(VimAction::WordEnd, &mut editor);
        assert_eq!(editor.textarea.cursor(), (0, 6));
        apply_vim_action(VimAction::WordBackward, &mut editor);
        assert_eq!(editor.textarea.cursor(), (0, 4));
    }

    #[test]
    fn change_and_delete_line_actions_match_vim_modes() {
        let mut change_line = editor("before after");
        change_line.textarea.move_cursor(CursorMove::End);
        apply_vim_action(VimAction::ChangeWholeLine, &mut change_line);
        assert_eq!(change_line.text(), "");
        assert_eq!(change_line.vim_mode, VimMode::Insert);

        let mut change_end = editor("before after");
        for _ in 0..7 {
            change_end.textarea.move_cursor(CursorMove::Forward);
        }
        apply_vim_action(VimAction::ChangeToLineEnd, &mut change_end);
        assert_eq!(change_end.text(), "before ");
        assert_eq!(change_end.vim_mode, VimMode::Insert);

        let mut delete_line = editor("before after");
        delete_line.vim_mode = VimMode::Normal;
        apply_vim_action(VimAction::DeleteWholeLine, &mut delete_line);
        assert_eq!(delete_line.text(), "");
        assert_eq!(delete_line.vim_mode, VimMode::Normal);

        let mut delete_end = editor("before after");
        delete_end.vim_mode = VimMode::Normal;
        for _ in 0..7 {
            delete_end.textarea.move_cursor(CursorMove::Forward);
        }
        apply_vim_action(VimAction::DeleteToLineEnd, &mut delete_end);
        assert_eq!(delete_end.text(), "before ");
        assert_eq!(delete_end.vim_mode, VimMode::Normal);
    }

    #[test]
    fn configured_action_flows_from_name_through_keymap_to_editor() {
        use crate::keymap::{KeyChord, KeyCode, KeyModifiers, Keymap};
        use std::collections::HashMap;

        // DeleteToLineStart is intentionally not represented by a fixed config field.
        let mut overrides = HashMap::new();
        overrides.insert("delete_to_line_start".to_owned(), "ctrl-k".to_owned());
        let keymap = Keymap::defaults()
            .with_vim_overrides(&overrides, VimMode::Insert)
            .unwrap();
        let key = KeyChord {
            code: KeyCode::Char('k'),
            modifiers: KeyModifiers {
                ctrl: true,
                alt: false,
            },
        };
        let action = keymap.resolve_vim(key, VimMode::Insert).unwrap();
        assert_eq!(action, VimAction::DeleteToLineStart);

        let mut editor = editor("before after");
        editor.textarea.move_cursor(CursorMove::End);
        apply_vim_action(action, &mut editor);
        assert_eq!(editor.text(), "");
    }

    #[test]
    fn big_word_motions_are_whitespace_delimited() {
        let mut editor = editor("foo-bar baz_qux");
        apply_vim_action(VimAction::WordForwardBig, &mut editor);
        assert_eq!(editor.textarea.cursor(), (0, 8));
        apply_vim_action(VimAction::WordBackwardBig, &mut editor);
        assert_eq!(editor.textarea.cursor(), (0, 0));
    }

    #[test]
    fn find_and_till_motions_seek_within_the_line() {
        let mut editor1 = editor("echo one two");
        assert!(apply_find_motion(&mut editor1, FindMotion::ForwardTo, 'o'));
        assert_eq!(editor1.textarea.cursor(), (0, 3));
        assert!(apply_find_motion(&mut editor1, FindMotion::ForwardTo, 'w'));
        assert_eq!(editor1.textarea.cursor(), (0, 10));
        assert!(apply_find_motion(&mut editor1, FindMotion::BackwardTo, 'o'));
        assert_eq!(editor1.textarea.cursor(), (0, 5));

        let mut editor2 = editor("echo one two");
        assert!(apply_find_motion(&mut editor2, FindMotion::ForwardTill, 'o'));
        assert_eq!(editor2.textarea.cursor(), (0, 2));
        assert!(!apply_find_motion(&mut editor2, FindMotion::ForwardTill, 'z'));
    }

    #[test]
    fn replace_char_substitutes_in_place() {
        let mut editor = editor("cat");
        apply_vim_action(VimAction::CursorRight, &mut editor);
        replace_char(&mut editor, 'u');
        assert_eq!(editor.text(), "cut");
        assert_eq!(editor.textarea.cursor(), (0, 1));
    }

    #[test]
    fn change_word_matches_vim_cw_quirk() {
        let mut editor = editor("foo bar");
        change_word(&mut editor);
        assert_eq!(editor.text(), " bar");
        assert_eq!(editor.vim_mode, VimMode::Insert);
    }

    #[test]
    fn change_word_big_is_whitespace_delimited() {
        let mut editor = editor("foo-bar baz");
        change_word_big(&mut editor);
        assert_eq!(editor.text(), " baz");
    }

    #[test]
    fn change_word_backward_deletes_previous_word() {
        let mut editor = editor("foo bar");
        editor.textarea.move_cursor(CursorMove::End);
        change_word_backward(&mut editor);
        assert_eq!(editor.text(), "foo ");
    }

    #[test]
    fn change_to_word_end_is_inclusive() {
        let mut editor = editor("foo bar");
        change_to_word_end(&mut editor);
        assert_eq!(editor.text(), " bar");
    }

    #[test]
    fn change_to_line_start_deletes_before_cursor() {
        let mut editor = editor("foo bar");
        for _ in 0..4 {
            editor.textarea.move_cursor(CursorMove::Forward);
        }
        change_to_line_start(&mut editor);
        assert_eq!(editor.text(), "bar");
    }

    #[test]
    fn change_over_find_deletes_inclusive_range() {
        let mut editor = editor("echo one two");
        assert!(change_over_find(&mut editor, FindMotion::ForwardTo, ' '));
        assert_eq!(editor.text(), "one two");
        assert_eq!(editor.vim_mode, VimMode::Insert);
    }

    #[test]
    fn delete_word_removes_up_to_next_word_start() {
        let mut editor = editor("foo bar baz");
        editor.vim_mode = VimMode::Normal;
        apply_vim_action(VimAction::DeleteWord, &mut editor);
        assert_eq!(editor.text(), "bar baz");
        assert_eq!(editor.vim_mode, VimMode::Normal);
    }

    #[test]
    fn delete_word_big_is_whitespace_delimited() {
        let mut editor = editor("foo-bar baz");
        apply_vim_action(VimAction::DeleteWordBig, &mut editor);
        assert_eq!(editor.text(), "baz");
    }

    #[test]
    fn open_line_below_inserts_newline_and_enters_insert_mode() {
        let mut editor = editor("first");
        apply_vim_action(VimAction::OpenLineBelow, &mut editor);
        assert_eq!(editor.text(), "first\n");
        assert_eq!(editor.vim_mode, VimMode::Insert);
        assert_eq!(editor.textarea.cursor(), (1, 0));
    }

    #[test]
    fn undo_restores_the_whole_word_changed_by_cw_in_one_step() {
        let mut editor = editor("foo bar");
        change_word(&mut editor); // deletes "foo", enters Insert mode
        insert_text(&mut editor, "quux");
        apply_vim_action(VimAction::ExitInsertMode, &mut editor);
        assert_eq!(editor.text(), "quux bar");

        apply_vim_action(VimAction::Undo, &mut editor);
        assert_eq!(editor.text(), "foo bar");
    }

    #[test]
    fn undo_and_redo_restore_whole_commands() {
        let mut editor = editor("foo bar");
        apply_vim_action(VimAction::DeleteWord, &mut editor);
        assert_eq!(editor.text(), "bar");

        apply_vim_action(VimAction::Undo, &mut editor);
        assert_eq!(editor.text(), "foo bar");

        apply_vim_action(VimAction::Redo, &mut editor);
        assert_eq!(editor.text(), "bar");
    }

    #[test]
    fn cursor_up_and_down_move_across_lines() {
        let mut editor = editor("first\nsecond\nthird");
        assert_eq!(editor.textarea.cursor(), (0, 0));
        apply_vim_action(VimAction::CursorDown, &mut editor);
        assert_eq!(editor.textarea.cursor().0, 1);
        apply_vim_action(VimAction::CursorDown, &mut editor);
        assert_eq!(editor.textarea.cursor().0, 2);
        apply_vim_action(VimAction::CursorUp, &mut editor);
        assert_eq!(editor.textarea.cursor().0, 1);
    }

    #[test]
    fn delete_whole_line_removes_the_line_on_multiline_buffers() {
        let mut middle = editor("first\nsecond\nthird");
        apply_vim_action(VimAction::CursorDown, &mut middle);
        apply_vim_action(VimAction::DeleteWholeLine, &mut middle);
        assert_eq!(middle.text(), "first\nthird");

        let mut last = editor("first\nsecond");
        apply_vim_action(VimAction::CursorDown, &mut last);
        apply_vim_action(VimAction::DeleteWholeLine, &mut last);
        assert_eq!(last.text(), "first");

        // Single-line buffers keep their prior (content-only) behavior.
        let mut single = editor("only");
        apply_vim_action(VimAction::DeleteWholeLine, &mut single);
        assert_eq!(single.text(), "");
    }

    #[test]
    fn insert_text_preserves_embedded_newlines() {
        let mut editor = editor("");
        insert_text(&mut editor, "one\ntwo\r\nthree");
        assert_eq!(editor.text(), "one\ntwo\nthree");
    }
}
