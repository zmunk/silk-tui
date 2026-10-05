//! Editor state and VimAction execution. Keymap → action resolution lives in keymap.rs;
//! action → editor behavior lives here.

use crate::keymap::VimMode;
use tui_textarea::TextArea;

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
        let lines: Vec<String> = text.lines().map(String::from).collect();
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
