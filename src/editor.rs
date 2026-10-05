//! Editor state and VimAction execution. Keymap → action resolution lives in keymap.rs;
//! action → editor behavior lives here.

use crate::keymap::{VimAction, VimMode};
use tui_textarea::{CursorMove, TextArea};

/// Effects that editor actions can produce, consumed by the app controller (§4 note).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorEffect {
    None,
    /// Execute the current command (Enter).
    Execute,
    /// Return the command without executing (q).
    KeepCommand,
    /// Copy current output to clipboard.
    CopyOutput,
    /// Copy current command to clipboard.
    CopyCommand,
    /// Toggle the error pane.
    ToggleErrorPane,
    /// Cancel warning / cancel.
    Cancel,
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

    /// Returns true if the text buffer is empty.
    pub fn is_empty(&self) -> bool {
        self.textarea.lines().iter().all(|l| l.is_empty())
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
        ExitInsertMode => editor.vim_mode = VimMode::Normal,
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
