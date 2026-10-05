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

/// Editor state: text buffer, cursor, Vim mode, undo/redo (§4, §16, §20).
pub struct EditorState {
    pub textarea: TextArea<'static>,
    pub vim_mode: VimMode,
}

impl EditorState {
    pub fn new() -> Self {
        Self {
            textarea: TextArea::default(),
            vim_mode: VimMode::Insert,
        }
    }

    /// Set the initial command text (from --query).
    pub fn set_text(&mut self, text: &str) {
        // `split` preserves both an empty buffer and trailing newlines.
        let lines: Vec<String> = text.split('\n').map(String::from).collect();
        self.textarea = TextArea::from(lines);
    }

    /// Get the current command text.
    pub fn text(&self) -> String {
        self.textarea.lines().join("\n")
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
        BeginningOfLine => editor.textarea.move_cursor(CursorMove::Head),
        EndOfLine => editor.textarea.move_cursor(CursorMove::End),
        WordForward => editor.textarea.move_cursor(CursorMove::WordForward),
        WordBackward => editor.textarea.move_cursor(CursorMove::WordBack),
        WordEnd => move_to_word_end(editor),
        EnterInsertMode => editor.vim_mode = VimMode::Insert,
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
            editor.textarea.move_cursor(CursorMove::Forward);
            editor.vim_mode = VimMode::Insert;
        }
        InsertAtBeginning => {
            editor.textarea.move_cursor(CursorMove::Head);
            editor.vim_mode = VimMode::Insert;
        }
        AppendAtEnd => {
            editor.textarea.move_cursor(CursorMove::End);
            editor.vim_mode = VimMode::Insert;
        }
        DeleteChar => {
            editor.textarea.delete_next_char();
        }
        DeleteToLineStart => {
            editor.textarea.delete_line_by_head();
        }
        ChangeWholeLine => {
            clear_current_line(editor);
            editor.vim_mode = VimMode::Insert;
        }
        ChangeToLineEnd => {
            editor.textarea.delete_line_by_end();
            editor.vim_mode = VimMode::Insert;
        }
        DeleteWholeLine => clear_current_line(editor),
        DeleteToLineEnd => {
            editor.textarea.delete_line_by_end();
        }
        Undo => {
            editor.textarea.undo();
        }
        Redo => {
            editor.textarea.redo();
        }
        KeepCommand => return EditorEffect::KeepCommand,
    }

    EditorEffect::None
}

fn clear_current_line(editor: &mut EditorState) {
    editor.textarea.move_cursor(CursorMove::Head);
    editor.textarea.delete_line_by_end();
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
}
