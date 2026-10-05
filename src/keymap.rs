//! Key parsing and mapping. Converts raw key chords into semantic VimAction / GlobalAction.
//!
//! Responsibilities:
//! - KeyChord representation and parsing from config strings (§19)
//! - VimAction and GlobalAction enums (§16, §21)
//! - VimKeymap (normal + insert bindings) and Keymap (global + vim) (§17–18)
//! - Default keybindings (§20–21)
//! - Conversion from crossterm KeyEvent → KeyChord (for runtime input)

use serde::{Deserialize, Deserializer, Serialize};
use std::collections::HashMap;
use std::str::FromStr;

// ---------------------------------------------------------------------------
// Key representation (§19)
// ---------------------------------------------------------------------------

/// Normalized key code for config and matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum KeyCode {
    Char(char),
    Enter,
    Esc,
    Backspace,
    Tab,
    Delete,
}

/// Modifiers for a key chord. Shift is implicit in Char case.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct KeyModifiers {
    pub ctrl: bool,
    pub alt: bool,
}

/// A single key chord: a key code plus modifiers (§19).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct KeyChord {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

/// Either a single key chord or the "gg" double-tap sequence.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum KeyOrSequence {
    Single(KeyChord),
    DoubleG,
    Double(KeyChord, KeyChord),
}

// ---------------------------------------------------------------------------
// Vim mode (§15, §20)
// ---------------------------------------------------------------------------

/// Vim input mode: Normal or Insert.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VimMode {
    Normal,
    Insert,
}

// ---------------------------------------------------------------------------
// Semantic actions (§16, §21)
// ---------------------------------------------------------------------------

/// Semantic Vim editor actions (§16).
///
/// Variant names map directly to snake_case config keys (§16, §18):
/// `CursorLeft` → `cursor_left`, `BeginningOfLine` → `beginning_of_line`, etc.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VimAction {
    CursorLeft,
    CursorRight,
    CursorUp,
    CursorDown,
    BeginningOfLine,
    EndOfLine,
    WordForward,
    WordBackward,
    WordForwardBig,
    WordBackwardBig,
    WordEnd,
    EnterInsertMode,
    ExitInsertMode,
    AppendInsertMode,
    InsertAtBeginning,
    AppendAtEnd,
    DeleteChar,
    DeleteToLineStart,
    ChangeWholeLine,
    ChangeToLineEnd,
    DeleteWholeLine,
    DeleteToLineEnd,
    DeleteWord,
    DeleteWordBig,
    OpenLineBelow,
    OpenLineAbove,
    JoinLines,
    ChangeChar,
    Undo,
    Redo,
    KeepCommand,
}

/// Global application actions (§21).
///
/// Variant names map directly to snake_case config keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GlobalAction {
    ExecuteCommand,
    Cancel,
    ToggleErrorPane,
    CopyOutput,
    CopyCommand,
    ScrollHalfPageDown,
    ScrollHalfPageUp,
    ScrollTop,
    ScrollBottom,
    RestoreLastValidCommand,
    KeepCommand,
}

// ---------------------------------------------------------------------------
// VimKeymap (§17–18)
// ---------------------------------------------------------------------------

/// Vim keymap: normal- and insert-mode bindings.
#[derive(Debug, Clone)]
pub struct VimKeymap {
    /// KeyChord → VimAction for normal mode.
    pub normal: HashMap<KeyChord, VimAction>,
    /// Two-key sequence → VimAction for normal mode.
    pub normal_sequences: HashMap<(KeyChord, KeyChord), VimAction>,
    /// KeyChord → VimAction for insert mode.
    pub insert: HashMap<KeyChord, VimAction>,
}

impl VimKeymap {
    /// Build the default Vim keymap (§20).
    pub fn defaults() -> Self {
        use KeyCode::*;
        use VimAction::*;

        fn ch(c: char) -> KeyChord {
            KeyChord {
                code: KeyCode::Char(c),
                modifiers: KeyModifiers::default(),
            }
        }
        fn ctrl(c: char) -> KeyChord {
            KeyChord {
                code: KeyCode::Char(c),
                modifiers: KeyModifiers {
                    ctrl: true,
                    alt: false,
                },
            }
        }

        // --- Normal mode defaults (§20) ---
        let mut normal: HashMap<KeyChord, VimAction> = HashMap::new();

        // Motions
        normal.insert(ch('h'), CursorLeft);
        normal.insert(ch('l'), CursorRight);
        normal.insert(ch('j'), CursorDown);
        normal.insert(ch('k'), CursorUp);
        normal.insert(ch('w'), WordForward);
        normal.insert(ch('b'), WordBackward);
        normal.insert(ch('W'), WordForwardBig);
        normal.insert(ch('B'), WordBackwardBig);
        normal.insert(ch('e'), WordEnd);
        normal.insert(ch('0'), BeginningOfLine);
        normal.insert(ch('$'), EndOfLine);

        // Transitions
        normal.insert(ch('i'), EnterInsertMode);
        normal.insert(ch('a'), AppendInsertMode);
        normal.insert(ch('I'), InsertAtBeginning);
        normal.insert(ch('A'), AppendAtEnd);

        // Editing
        normal.insert(ch('x'), DeleteChar);
        normal.insert(ch('C'), ChangeToLineEnd);
        normal.insert(ch('D'), DeleteToLineEnd);
        normal.insert(ch('u'), Undo);
        normal.insert(ctrl('r'), Redo);

        // Exit without execution
        normal.insert(ch('q'), KeepCommand);

        // Open a new line below and enter Insert mode.
        normal.insert(ch('o'), OpenLineBelow);
        normal.insert(ch('O'), OpenLineAbove);
        normal.insert(ch('J'), JoinLines);
        normal.insert(ch('s'), ChangeChar);

        let mut normal_sequences = HashMap::new();
        normal_sequences.insert((ch('d'), ch('d')), DeleteWholeLine);
        normal_sequences.insert((ch('d'), ch('w')), DeleteWord);
        normal_sequences.insert((ch('d'), ch('W')), DeleteWordBig);

        // --- Insert mode defaults (§20) ---
        let mut insert: HashMap<KeyChord, VimAction> = HashMap::new();

        // Ctrl-U clears from the cursor to the beginning of the line.
        insert.insert(ctrl('u'), DeleteToLineStart);

        // Esc → Normal mode (handled as a special transition, but still mappable)
        insert.insert(
            KeyChord {
                code: Esc,
                modifiers: KeyModifiers::default(),
            },
            ExitInsertMode,
        );

        // Note: Enter, Ctrl-C, Ctrl-E in insert mode are global actions, not Vim actions.
        // They are resolved by the global keymap first.

        Self {
            normal,
            normal_sequences,
            insert,
        }
    }
}

// ---------------------------------------------------------------------------
// Keymap (runtime lookup)
// ---------------------------------------------------------------------------

/// Runtime keymap for resolving input events to actions.
#[derive(Debug, Clone)]
pub struct Keymap {
    /// Single-key → GlobalAction for global bindings.
    pub globals: HashMap<KeyChord, GlobalAction>,
    /// What action does "gg" trigger? (None if unbound)
    pub gg_action: Option<GlobalAction>,
    /// Vim-mode-specific bindings.
    pub vim: VimKeymap,
}

impl Keymap {
    /// Build the default keymap (global + vim defaults).
    pub fn defaults() -> Self {
        use GlobalAction::*;
        use KeyCode::*;

        fn ch(c: char) -> KeyChord {
            KeyChord {
                code: KeyCode::Char(c),
                modifiers: KeyModifiers::default(),
            }
        }
        fn ctrl(c: char) -> KeyChord {
            KeyChord {
                code: KeyCode::Char(c),
                modifiers: KeyModifiers {
                    ctrl: true,
                    alt: false,
                },
            }
        }
        fn alt(c: char) -> KeyChord {
            KeyChord {
                code: KeyCode::Char(c),
                modifiers: KeyModifiers {
                    ctrl: false,
                    alt: true,
                },
            }
        }

        let mut globals: HashMap<KeyChord, GlobalAction> = HashMap::new();

        // §21 defaults
        globals.insert(
            KeyChord {
                code: Enter,
                modifiers: KeyModifiers::default(),
            },
            ExecuteCommand,
        );
        globals.insert(ctrl('c'), Cancel);
        globals.insert(ctrl('q'), KeepCommand);
        globals.insert(ctrl('e'), ToggleErrorPane);
        globals.insert(ctrl('y'), CopyOutput);
        globals.insert(alt('y'), CopyCommand);
        globals.insert(ctrl('d'), ScrollHalfPageDown);
        globals.insert(ctrl('u'), ScrollHalfPageUp);
        globals.insert(ch('G'), ScrollBottom);
        globals.insert(alt('u'), RestoreLastValidCommand);

        Self {
            globals,
            gg_action: Some(ScrollTop), // "gg" → ScrollTop
            vim: VimKeymap::defaults(),
        }
    }

    /// Resolve a global action from a key chord, considering previous key for "gg" sequences.
    pub fn resolve_global(
        &self,
        key: KeyChord,
        prev_key: Option<KeyChord>,
    ) -> Option<GlobalAction> {
        // Check "gg" sequence first
        if let Some(action) = self.gg_action {
            if key.code == KeyCode::Char('g')
                && key.modifiers.ctrl == false
                && key.modifiers.alt == false
            {
                if let Some(prev) = prev_key {
                    if prev.code == KeyCode::Char('g')
                        && prev.modifiers.ctrl == false
                        && prev.modifiers.alt == false
                    {
                        return Some(action);
                    }
                }
            }
        }
        self.globals.get(&key).copied()
    }

    /// Resolve a Vim action from a key chord and mode.
    pub fn resolve_vim(&self, key: KeyChord, mode: VimMode) -> Option<VimAction> {
        match mode {
            VimMode::Normal => self.vim.normal.get(&key).copied(),
            VimMode::Insert => self.vim.insert.get(&key).copied(),
        }
    }

    pub fn resolve_vim_sequence(
        &self,
        key: KeyChord,
        previous: Option<KeyChord>,
        mode: VimMode,
    ) -> Option<VimAction> {
        if mode != VimMode::Normal {
            return None;
        }
        previous.and_then(|first| self.vim.normal_sequences.get(&(first, key)).copied())
    }

    pub fn is_vim_sequence_prefix(&self, key: KeyChord, mode: VimMode) -> bool {
        mode == VimMode::Normal
            && self
                .vim
                .normal_sequences
                .keys()
                .any(|(first, _)| *first == key)
    }

    /// Merge user-specified global bindings over the defaults.
    ///
    /// `overrides` is action_name → key_string (e.g., "copy_output" → "ctrl-y").
    /// Returns a new Keymap with defaults + overrides applied.
    pub fn with_global_overrides(
        &self,
        overrides: &HashMap<String, String>,
    ) -> anyhow::Result<Self> {
        let mut keymap = self.clone();

        for (action_name, key_str) in overrides {
            let action: GlobalAction = action_name
                .parse()
                .map_err(|e| anyhow::anyhow!("unknown global action '{}': {}", action_name, e))?;
            let binding: KeyOrSequence = key_str.parse().map_err(|e| {
                anyhow::anyhow!("invalid key '{}' for '{}': {}", key_str, action_name, e)
            })?;

            // Remove any existing binding for this action
            keymap.globals.retain(|_, a| *a != action);
            if keymap.gg_action == Some(action) {
                keymap.gg_action = None;
            }

            match binding {
                KeyOrSequence::Single(chord) => {
                    keymap.globals.insert(chord, action);
                }
                KeyOrSequence::DoubleG => {
                    keymap.gg_action = Some(action);
                }
                KeyOrSequence::Double(_, _) => {
                    anyhow::bail!("only the `gg` sequence is supported for global actions");
                }
            }
        }

        Ok(keymap)
    }

    /// Merge user-specified Vim bindings over the defaults.
    ///
    /// `overrides` is action_name → key_string (e.g., "end_of_line" → ";").
    /// `mode` specifies which mode map to update.
    pub fn with_vim_overrides(
        &self,
        overrides: &HashMap<String, String>,
        mode: VimMode,
    ) -> anyhow::Result<Self> {
        let mut keymap = self.clone();
        for (action_name, key_str) in overrides {
            let action: VimAction = action_name
                .parse()
                .map_err(|e| anyhow::anyhow!("unknown Vim action '{}': {}", action_name, e))?;
            let binding: KeyOrSequence = key_str.parse().map_err(|e| {
                anyhow::anyhow!("invalid key '{}' for '{}': {}", key_str, action_name, e)
            })?;

            // Remove any existing binding for this action in this mode.
            match mode {
                VimMode::Normal => {
                    keymap.vim.normal.retain(|_, mapped| *mapped != action);
                    keymap
                        .vim
                        .normal_sequences
                        .retain(|_, mapped| *mapped != action);
                }
                VimMode::Insert => keymap.vim.insert.retain(|_, mapped| *mapped != action),
            }

            match binding {
                KeyOrSequence::Single(chord) => {
                    match mode {
                        VimMode::Normal => keymap.vim.normal.insert(chord, action),
                        VimMode::Insert => keymap.vim.insert.insert(chord, action),
                    };
                }
                KeyOrSequence::DoubleG if mode == VimMode::Normal => {
                    let g: KeyChord = "g".parse().expect("literal key is valid");
                    keymap.vim.normal_sequences.insert((g, g), action);
                }
                KeyOrSequence::Double(first, second) if mode == VimMode::Normal => {
                    keymap.vim.normal_sequences.insert((first, second), action);
                }
                KeyOrSequence::DoubleG | KeyOrSequence::Double(_, _) => {
                    anyhow::bail!("key sequences are only supported in Vim normal mode");
                }
            }
        }

        Ok(keymap)
    }
}

// ---------------------------------------------------------------------------
// Key string parsing (§19)
// ---------------------------------------------------------------------------

/// Error returned when a key string cannot be parsed.
#[derive(Debug, Clone)]
pub struct ParseKeyError {
    pub input: String,
    pub reason: String,
}

impl std::fmt::Display for ParseKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "invalid key '{}': {}", self.input, self.reason)
    }
}

impl std::error::Error for ParseKeyError {}

impl FromStr for KeyChord {
    type Err = ParseKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = |reason: &str| ParseKeyError {
            input: s.to_string(),
            reason: reason.to_string(),
        };

        match s {
            "enter" => {
                return Ok(KeyChord {
                    code: KeyCode::Enter,
                    modifiers: KeyModifiers::default(),
                });
            }
            "escape" | "esc" => {
                return Ok(KeyChord {
                    code: KeyCode::Esc,
                    modifiers: KeyModifiers::default(),
                });
            }
            "backspace" => {
                return Ok(KeyChord {
                    code: KeyCode::Backspace,
                    modifiers: KeyModifiers::default(),
                });
            }
            "tab" => {
                return Ok(KeyChord {
                    code: KeyCode::Tab,
                    modifiers: KeyModifiers::default(),
                });
            }
            "delete" => {
                return Ok(KeyChord {
                    code: KeyCode::Delete,
                    modifiers: KeyModifiers::default(),
                });
            }
            _ => {}
        }

        // ctrl-<char>
        if let Some(rest) = s.strip_prefix("ctrl-") {
            if rest.len() == 1 {
                let c = rest.chars().next().unwrap();
                if c.is_ascii_lowercase() {
                    return Ok(KeyChord {
                        code: KeyCode::Char(c),
                        modifiers: KeyModifiers {
                            ctrl: true,
                            alt: false,
                        },
                    });
                }
            }
            return Err(err("expected ctrl-<lowercase-letter> (e.g. ctrl-r)"));
        }

        // alt-<char>
        if let Some(rest) = s.strip_prefix("alt-") {
            if rest.len() == 1 {
                let c = rest.chars().next().unwrap();
                if c.is_ascii_lowercase() {
                    return Ok(KeyChord {
                        code: KeyCode::Char(c),
                        modifiers: KeyModifiers {
                            ctrl: false,
                            alt: true,
                        },
                    });
                }
            }
            return Err(err("expected alt-<lowercase-letter> (e.g. alt-y)"));
        }

        // Single character
        if s.chars().count() == 1 {
            let c = s.chars().next().unwrap();
            return Ok(KeyChord {
                code: KeyCode::Char(c),
                modifiers: KeyModifiers::default(),
            });
        }

        Err(err("unrecognized key format"))
    }
}

impl FromStr for KeyOrSequence {
    type Err = ParseKeyError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s == "gg" {
            return Ok(KeyOrSequence::DoubleG);
        }
        if s.chars().count() == 2 {
            let mut characters = s.chars();
            let first = characters.next().expect("length checked").to_string();
            let second = characters.next().expect("length checked").to_string();
            return Ok(KeyOrSequence::Double(first.parse()?, second.parse()?));
        }
        s.parse::<KeyChord>().map(KeyOrSequence::Single)
    }
}

// ---------------------------------------------------------------------------
// Action name parsing (snake_case string → enum variant)
// ---------------------------------------------------------------------------

/// Error for unknown action names.
#[derive(Debug, Clone)]
pub struct ParseActionError {
    pub input: String,
}

impl std::fmt::Display for ParseActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown action: '{}'", self.input)
    }
}

impl std::error::Error for ParseActionError {}

/// Deserialize an action name through its Serde representation. Because the action
/// enums use `rename_all = "snake_case"`, newly added variants automatically become
/// available to configuration without another parser match arm.
fn parse_action<T>(s: &str) -> Result<T, ParseActionError>
where
    T: for<'de> Deserialize<'de>,
{
    toml::Value::String(s.to_string())
        .try_into()
        .map_err(|_| ParseActionError {
            input: s.to_string(),
        })
}

impl FromStr for VimAction {
    type Err = ParseActionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_action(s)
    }
}

impl FromStr for GlobalAction {
    type Err = ParseActionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_action(s)
    }
}

// ---------------------------------------------------------------------------
// Serde support: deserialize KeyChord / KeyOrSequence from strings
// ---------------------------------------------------------------------------

impl<'de> Deserialize<'de> for KeyChord {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

impl Serialize for KeyChord {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        // Serialize back to string form for round-tripping
        self.to_string().serialize(serializer)
    }
}

impl std::fmt::Display for KeyChord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.modifiers.ctrl {
            write!(f, "ctrl-")?;
        }
        if self.modifiers.alt {
            write!(f, "alt-")?;
        }
        match self.code {
            KeyCode::Char(c) => write!(f, "{}", c),
            KeyCode::Enter => write!(f, "enter"),
            KeyCode::Esc => write!(f, "escape"),
            KeyCode::Backspace => write!(f, "backspace"),
            KeyCode::Tab => write!(f, "tab"),
            KeyCode::Delete => write!(f, "delete"),
        }
    }
}

// ---------------------------------------------------------------------------
// Conversion from crossterm KeyEvent → KeyChord (for runtime input)
// ---------------------------------------------------------------------------

/// Convert a crossterm `KeyEvent` into our normalized `KeyChord`.
///
/// Shift is normalized into character case (e.g., Shift+G → Char('G') with no shift modifier).
pub fn key_chord_from_crossterm(event: crossterm::event::KeyEvent) -> KeyChord {
    use crossterm::event::{KeyCode as CKeyCode, KeyModifiers as CModifiers};

    let mods = event.modifiers;
    let ctrl = mods.contains(CModifiers::CONTROL);
    let alt = mods.contains(CModifiers::ALT);
    let shift = mods.contains(CModifiers::SHIFT);

    let code = match event.code {
        CKeyCode::Char(c) => {
            // Normalize shift into character case
            if shift && c.is_ascii_lowercase() {
                KeyCode::Char(c.to_ascii_uppercase())
            } else {
                KeyCode::Char(c)
            }
        }
        CKeyCode::Enter => KeyCode::Enter,
        CKeyCode::Esc => KeyCode::Esc,
        CKeyCode::Backspace => KeyCode::Backspace,
        CKeyCode::Tab => KeyCode::Tab,
        CKeyCode::Delete => KeyCode::Delete,
        _ => {
            // For keys we don't handle specially, try to represent as Char if possible.
            // Fall back to a null char — caller should check.
            KeyCode::Char('\0')
        }
    };

    KeyChord {
        code,
        modifiers: KeyModifiers { ctrl, alt },
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- KeyChord parsing ---

    #[test]
    fn parse_single_char() {
        let k: KeyChord = "a".parse().unwrap();
        assert_eq!(k.code, KeyCode::Char('a'));
        assert!(!k.modifiers.ctrl);
        assert!(!k.modifiers.alt);

        let k: KeyChord = ";".parse().unwrap();
        assert_eq!(k.code, KeyCode::Char(';'));

        let k: KeyChord = "$".parse().unwrap();
        assert_eq!(k.code, KeyCode::Char('$'));

        let k: KeyChord = "A".parse().unwrap();
        assert_eq!(k.code, KeyCode::Char('A'));
    }

    #[test]
    fn parse_named_keys() {
        let k: KeyChord = "enter".parse().unwrap();
        assert_eq!(k.code, KeyCode::Enter);

        let k: KeyChord = "escape".parse().unwrap();
        assert_eq!(k.code, KeyCode::Esc);

        let k: KeyChord = "esc".parse().unwrap();
        assert_eq!(k.code, KeyCode::Esc);

        let k: KeyChord = "backspace".parse().unwrap();
        assert_eq!(k.code, KeyCode::Backspace);

        let k: KeyChord = "tab".parse().unwrap();
        assert_eq!(k.code, KeyCode::Tab);
    }

    #[test]
    fn parse_ctrl_keys() {
        let k: KeyChord = "ctrl-r".parse().unwrap();
        assert_eq!(k.code, KeyCode::Char('r'));
        assert!(k.modifiers.ctrl);
        assert!(!k.modifiers.alt);

        let k: KeyChord = "ctrl-d".parse().unwrap();
        assert_eq!(k.code, KeyCode::Char('d'));
        assert!(k.modifiers.ctrl);
    }

    #[test]
    fn parse_alt_keys() {
        let k: KeyChord = "alt-y".parse().unwrap();
        assert_eq!(k.code, KeyCode::Char('y'));
        assert!(!k.modifiers.ctrl);
        assert!(k.modifiers.alt);
    }

    #[test]
    fn parse_key_or_sequence() {
        let k: KeyOrSequence = "gg".parse().unwrap();
        assert_eq!(k, KeyOrSequence::DoubleG);

        let k: KeyOrSequence = "cc".parse().unwrap();
        assert_eq!(
            k,
            KeyOrSequence::Double(
                "c".parse::<KeyChord>().unwrap(),
                "c".parse::<KeyChord>().unwrap()
            )
        );

        let k: KeyOrSequence = "ctrl-y".parse().unwrap();
        assert_eq!(
            k,
            KeyOrSequence::Single(KeyChord {
                code: KeyCode::Char('y'),
                modifiers: KeyModifiers {
                    ctrl: true,
                    alt: false,
                },
            })
        );
    }

    #[test]
    fn parse_invalid_keys() {
        assert!("ctrl-".parse::<KeyChord>().is_err());
        assert!("ctrl-ab".parse::<KeyChord>().is_err());
        assert!("alt-AB".parse::<KeyChord>().is_err());
        assert!("unknown".parse::<KeyChord>().is_err());
    }

    // --- Action name parsing ---

    #[test]
    fn parse_vim_action_names() {
        assert_eq!(
            "cursor_left".parse::<VimAction>().unwrap(),
            VimAction::CursorLeft
        );
        assert_eq!(
            "beginning_of_line".parse::<VimAction>().unwrap(),
            VimAction::BeginningOfLine
        );
        assert_eq!(
            "end_of_line".parse::<VimAction>().unwrap(),
            VimAction::EndOfLine
        );
        assert_eq!(
            "enter_insert_mode".parse::<VimAction>().unwrap(),
            VimAction::EnterInsertMode
        );
        assert_eq!(
            "exit_insert_mode".parse::<VimAction>().unwrap(),
            VimAction::ExitInsertMode
        );
        assert_eq!(
            "delete_char".parse::<VimAction>().unwrap(),
            VimAction::DeleteChar
        );
        assert_eq!("undo".parse::<VimAction>().unwrap(), VimAction::Undo);
        assert_eq!("redo".parse::<VimAction>().unwrap(), VimAction::Redo);
        assert_eq!(
            "keep_command".parse::<VimAction>().unwrap(),
            VimAction::KeepCommand
        );
    }

    #[test]
    fn parse_global_action_names() {
        assert_eq!(
            "execute_command".parse::<GlobalAction>().unwrap(),
            GlobalAction::ExecuteCommand
        );
        assert_eq!(
            "cancel".parse::<GlobalAction>().unwrap(),
            GlobalAction::Cancel
        );
        assert_eq!(
            "toggle_error_pane".parse::<GlobalAction>().unwrap(),
            GlobalAction::ToggleErrorPane
        );
        assert_eq!(
            "copy_output".parse::<GlobalAction>().unwrap(),
            GlobalAction::CopyOutput
        );
        assert_eq!(
            "copy_command".parse::<GlobalAction>().unwrap(),
            GlobalAction::CopyCommand
        );
        assert_eq!(
            "scroll_top".parse::<GlobalAction>().unwrap(),
            GlobalAction::ScrollTop
        );
        assert_eq!(
            "scroll_bottom".parse::<GlobalAction>().unwrap(),
            GlobalAction::ScrollBottom
        );
    }

    // --- Default keymap ---

    #[test]
    fn default_keymap_has_all_global_actions() {
        let km = Keymap::defaults();
        // Every GlobalAction variant should be reachable
        let actions: Vec<GlobalAction> = km.globals.values().copied().collect();
        // ScrollTop is via gg_action
        assert!(actions.contains(&GlobalAction::ExecuteCommand));
        assert!(actions.contains(&GlobalAction::Cancel));
        assert!(actions.contains(&GlobalAction::ToggleErrorPane));
        assert!(actions.contains(&GlobalAction::CopyOutput));
        assert!(actions.contains(&GlobalAction::CopyCommand));
        assert!(actions.contains(&GlobalAction::ScrollHalfPageDown));
        assert!(actions.contains(&GlobalAction::ScrollHalfPageUp));
        assert!(actions.contains(&GlobalAction::ScrollBottom));
        assert!(actions.contains(&GlobalAction::RestoreLastValidCommand));
        assert_eq!(km.gg_action, Some(GlobalAction::ScrollTop));
    }

    #[test]
    fn default_vim_keymap_has_all_actions() {
        let km = VimKeymap::defaults();
        // Check a few key normal-mode bindings
        assert_eq!(
            km.normal.get(&KeyChord {
                code: KeyCode::Char('h'),
                modifiers: KeyModifiers::default(),
            }),
            Some(&VimAction::CursorLeft)
        );
        assert_eq!(
            km.normal.get(&KeyChord {
                code: KeyCode::Char('i'),
                modifiers: KeyModifiers::default(),
            }),
            Some(&VimAction::EnterInsertMode)
        );
        assert_eq!(
            km.normal.get(&KeyChord {
                code: KeyCode::Char('q'),
                modifiers: KeyModifiers::default(),
            }),
            Some(&VimAction::KeepCommand)
        );
        assert_eq!(
            km.normal.get(&KeyChord {
                code: KeyCode::Char('x'),
                modifiers: KeyModifiers::default(),
            }),
            Some(&VimAction::DeleteChar)
        );
        assert_eq!(
            km.insert.get(&KeyChord {
                code: KeyCode::Char('u'),
                modifiers: KeyModifiers {
                    ctrl: true,
                    alt: false,
                },
            }),
            Some(&VimAction::DeleteToLineStart)
        );
        let d: KeyChord = "d".parse().unwrap();
        assert_eq!(
            km.normal_sequences.get(&(d, d)),
            Some(&VimAction::DeleteWholeLine)
        );
        assert_eq!(
            km.normal.get(&"C".parse().unwrap()),
            Some(&VimAction::ChangeToLineEnd)
        );
        assert_eq!(
            km.normal.get(&"D".parse().unwrap()),
            Some(&VimAction::DeleteToLineEnd)
        );
    }

    // --- Override merging ---

    #[test]
    fn global_override_replaces_default() {
        let km = Keymap::defaults();
        let mut overrides = HashMap::new();
        // Rebind copy_output to "ctrl-o" instead of "ctrl-y"
        overrides.insert("copy_output".to_string(), "ctrl-o".to_string());

        let merged = km.with_global_overrides(&overrides).unwrap();

        // Old binding should be gone
        assert_eq!(
            merged.globals.get(&KeyChord {
                code: KeyCode::Char('y'),
                modifiers: KeyModifiers {
                    ctrl: true,
                    alt: false,
                },
            }),
            None
        );
        // New binding should be present
        assert_eq!(
            merged.globals.get(&KeyChord {
                code: KeyCode::Char('o'),
                modifiers: KeyModifiers {
                    ctrl: true,
                    alt: false,
                },
            }),
            Some(&GlobalAction::CopyOutput)
        );
    }

    #[test]
    fn vim_override_replaces_default_and_leaves_others() {
        let km = Keymap::defaults();
        let mut overrides = HashMap::new();
        // Rebind end_of_line to ";"
        overrides.insert("end_of_line".to_string(), ";".to_string());

        let merged = km.with_vim_overrides(&overrides, VimMode::Normal).unwrap();

        // Old binding ($) should be gone
        assert_eq!(
            merged.vim.normal.get(&KeyChord {
                code: KeyCode::Char('$'),
                modifiers: KeyModifiers::default(),
            }),
            None
        );
        // New binding (;) should invoke EndOfLine
        assert_eq!(
            merged.vim.normal.get(&KeyChord {
                code: KeyCode::Char(';'),
                modifiers: KeyModifiers::default(),
            }),
            Some(&VimAction::EndOfLine)
        );
        // Other bindings should be intact
        assert_eq!(
            merged.vim.normal.get(&KeyChord {
                code: KeyCode::Char('h'),
                modifiers: KeyModifiers::default(),
            }),
            Some(&VimAction::CursorLeft)
        );
    }

    #[test]
    fn gg_override_works() {
        let km = Keymap::defaults();
        assert_eq!(km.gg_action, Some(GlobalAction::ScrollTop));

        // Rebind scroll_top to "ctrl-t"
        let mut overrides = HashMap::new();
        overrides.insert("scroll_top".to_string(), "ctrl-t".to_string());
        let merged = km.with_global_overrides(&overrides).unwrap();

        // gg should no longer trigger ScrollTop
        assert_eq!(merged.gg_action, None);
        // ctrl-t should now trigger ScrollTop
        assert_eq!(
            merged.globals.get(&KeyChord {
                code: KeyCode::Char('t'),
                modifiers: KeyModifiers {
                    ctrl: true,
                    alt: false,
                },
            }),
            Some(&GlobalAction::ScrollTop)
        );
    }

    // --- Crossterm conversion ---

    #[test]
    fn crossterm_char_conversion() {
        use crossterm::event::{KeyCode as CKeyCode, KeyEvent, KeyModifiers as CModifiers};

        let ev = KeyEvent::new(CKeyCode::Char('a'), CModifiers::empty());
        let kc = key_chord_from_crossterm(ev);
        assert_eq!(kc.code, KeyCode::Char('a'));
        assert!(!kc.modifiers.ctrl);
        assert!(!kc.modifiers.alt);

        // Shift normalizes to uppercase
        let ev = KeyEvent::new(CKeyCode::Char('g'), CModifiers::SHIFT);
        let kc = key_chord_from_crossterm(ev);
        assert_eq!(kc.code, KeyCode::Char('G'));
        assert!(!kc.modifiers.ctrl);
        assert!(!kc.modifiers.alt);

        // Ctrl
        let ev = KeyEvent::new(CKeyCode::Char('c'), CModifiers::CONTROL);
        let kc = key_chord_from_crossterm(ev);
        assert_eq!(kc.code, KeyCode::Char('c'));
        assert!(kc.modifiers.ctrl);
    }

    #[test]
    fn crossterm_special_keys() {
        use crossterm::event::{KeyCode as CKeyCode, KeyEvent, KeyModifiers as CModifiers};

        let ev = KeyEvent::new(CKeyCode::Enter, CModifiers::empty());
        let kc = key_chord_from_crossterm(ev);
        assert_eq!(kc.code, KeyCode::Enter);

        let ev = KeyEvent::new(CKeyCode::Esc, CModifiers::empty());
        let kc = key_chord_from_crossterm(ev);
        assert_eq!(kc.code, KeyCode::Esc);
    }
}
