//! Configuration loading and merging (§28).
//!
//! Loads from `~/.config/silk/config.toml` (or XDG_CONFIG_HOME), merges user config
//! over built-in defaults. Silk must run without any config file present.
//!
//! Config parsing is isolated from runtime — no config types leak into UI/evaluator.

use crate::keymap::{Keymap, VimMode};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;

// ---------------------------------------------------------------------------
// Input mode (§15)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Vim,
    Regular,
}

// ---------------------------------------------------------------------------
// Raw TOML deserialization types
// ---------------------------------------------------------------------------

/// Raw config as deserialized from TOML. All keybinding values are strings
/// that get parsed into KeyChord / KeyOrSequence during resolution.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct RawConfig {
    input_mode: String,
    shell: String,
    debounce_ms: u64,
    clipboard_command: String,
    #[serde(rename = "keybindings")]
    keybindings: HashMap<String, String>,
    #[serde(rename = "environment")]
    environment: HashMap<String, String>,
    #[serde(rename = "vim_keybindings")]
    vim_keybindings: RawVimKeybindings,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
struct RawVimKeybindings {
    normal: HashMap<String, String>,
    insert: HashMap<String, String>,
}

// Default impls for RawConfig
impl Default for RawConfig {
    fn default() -> Self {
        Self {
            input_mode: "vim".to_string(),
            shell: "zsh".to_string(),
            debounce_ms: 100,
            clipboard_command: default_clipboard_command(),
            keybindings: HashMap::new(),
            environment: HashMap::new(),
            vim_keybindings: RawVimKeybindings::default(),
        }
    }
}

impl Default for RawVimKeybindings {
    fn default() -> Self {
        Self {
            normal: HashMap::new(),
            insert: HashMap::new(),
        }
    }
}

// ---------------------------------------------------------------------------
// Resolved configuration
// ---------------------------------------------------------------------------

/// Resolved configuration with parsed keybindings, ready for runtime use.
#[derive(Debug, Clone)]
pub struct Config {
    pub input_mode: InputMode,
    pub shell: String,
    pub debounce_ms: u64,
    pub clipboard_command: String,
    pub environment: HashMap<String, String>,
    /// Resolved keymap (global + vim) built from defaults merged with user overrides.
    pub keymap: Keymap,
}

impl Config {
    /// Load configuration: read user config file (if present), merge over defaults.
    ///
    /// Searches `$XDG_CONFIG_HOME/silk/config.toml` (falling back to
    /// `~/.config/silk/config.toml`). If no file is found, returns defaults.
    pub fn load() -> anyhow::Result<Self> {
        let config_path = config_path();

        let raw = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            toml::from_str::<RawConfig>(&content)?
        } else {
            RawConfig::default()
        };

        raw.resolve()
    }
}

impl RawConfig {
    /// Resolve raw config into a runtime `Config`, parsing key strings and merging
    /// user overrides on top of the default keymap.
    fn resolve(&self) -> anyhow::Result<Config> {
        let input_mode = match self.input_mode.as_str() {
            "vim" => InputMode::Vim,
            "regular" => InputMode::Regular,
            other => anyhow::bail!(
                "unknown input_mode '{}': expected 'vim' or 'regular'",
                other
            ),
        };

        // Start with the default keymap
        let mut keymap = Keymap::defaults();

        // Merge global keybinding overrides
        if !self.keybindings.is_empty() {
            keymap = keymap.with_global_overrides(&self.keybindings)?;
        }

        // Merge Vim normal-mode overrides
        if !self.vim_keybindings.normal.is_empty() {
            keymap = keymap.with_vim_overrides(&self.vim_keybindings.normal, VimMode::Normal)?;
        }

        // Merge Vim insert-mode overrides
        if !self.vim_keybindings.insert.is_empty() {
            keymap = keymap.with_vim_overrides(&self.vim_keybindings.insert, VimMode::Insert)?;
        }

        Ok(Config {
            input_mode,
            shell: self.shell.clone(),
            debounce_ms: self.debounce_ms,
            clipboard_command: self.clipboard_command.clone(),
            environment: self.environment.clone(),
            keymap,
        })
    }
}

// ---------------------------------------------------------------------------
// Config path resolution
// ---------------------------------------------------------------------------

/// Return the path to the user config file.
///
/// Uses `$XDG_CONFIG_HOME/silk/config.toml` if set, otherwise
/// `~/.config/silk/config.toml`.
fn config_path() -> PathBuf {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        let mut p = PathBuf::from(dir);
        p.push("silk");
        p.push("config.toml");
        return p;
    }

    let home = std::env::var("HOME").unwrap_or_else(|_| "/home/user".to_string());
    let mut p = PathBuf::from(home);
    p.push(".config");
    p.push("silk");
    p.push("config.toml");
    p
}

// ---------------------------------------------------------------------------
// Platform-appropriate default clipboard command
// ---------------------------------------------------------------------------

fn default_clipboard_command() -> String {
    #[cfg(target_os = "macos")]
    {
        "pbcopy".to_string()
    }
    #[cfg(target_os = "linux")]
    {
        // Try to detect Wayland vs X11 at runtime? For default, prefer wl-copy
        // as it's common on modern Linux, but xclip is a safe fallback.
        // We'll use a simple heuristic: if WAYLAND_DISPLAY is set, use wl-copy.
        // In practice, users can override via config.
        if std::env::var("WAYLAND_DISPLAY").is_ok() {
            "wl-copy".to_string()
        } else {
            "xclip -selection clipboard".to_string()
        }
    }
    #[cfg(target_os = "windows")]
    {
        "clip.exe".to_string()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "pbcopy".to_string()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: resolve a raw config from TOML snippet.
    fn parse(toml_str: &str) -> anyhow::Result<Config> {
        let raw: RawConfig = toml::from_str(toml_str)?;
        raw.resolve()
    }

    #[test]
    fn defaults_without_config_file() {
        let config = parse("").unwrap();
        assert_eq!(config.input_mode, InputMode::Vim);
        assert_eq!(config.shell, "zsh");
        assert_eq!(config.debounce_ms, 100);
        // Keymap should have defaults
        assert!(config.keymap.globals.len() > 0);
        assert!(config.keymap.vim.normal.len() > 0);
    }

    #[test]
    fn input_mode_regular() {
        let config = parse(r#"input_mode = "regular""#).unwrap();
        assert_eq!(config.input_mode, InputMode::Regular);
    }

    #[test]
    fn invalid_input_mode() {
        let result = parse(r#"input_mode = "emacs""#);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("unknown input_mode")
        );
    }

    #[test]
    fn custom_shell_and_debounce() {
        let config = parse(
            r#"
            shell = "bash"
            debounce_ms = 200
            "#,
        )
        .unwrap();
        assert_eq!(config.shell, "bash");
        assert_eq!(config.debounce_ms, 200);
    }

    #[test]
    fn environment_variables() {
        let config = parse(
            r#"
            [environment]
            NO_COLOR = "1"
            RG_WRAPPER_DISABLE = "1"
            "#,
        )
        .unwrap();
        assert_eq!(config.environment.get("NO_COLOR").unwrap(), "1");
        assert_eq!(config.environment.get("RG_WRAPPER_DISABLE").unwrap(), "1");
    }

    #[test]
    fn global_keybinding_override() {
        let config = parse(
            r#"
            [keybindings]
            copy_output = "ctrl-o"
            "#,
        )
        .unwrap();

        use crate::keymap::{KeyChord, KeyCode, KeyModifiers};
        // Old default (ctrl-y) should not map to CopyOutput
        let old = KeyChord {
            code: KeyCode::Char('y'),
            modifiers: KeyModifiers {
                ctrl: true,
                alt: false,
            },
        };
        assert!(!config.keymap.globals.contains_key(&old));

        // New binding should map to CopyOutput
        let new = KeyChord {
            code: KeyCode::Char('o'),
            modifiers: KeyModifiers {
                ctrl: true,
                alt: false,
            },
        };
        assert_eq!(
            config.keymap.globals.get(&new),
            Some(&crate::keymap::GlobalAction::CopyOutput)
        );
    }

    #[test]
    fn vim_keybinding_override_normal() {
        let config = parse(
            r#"
            [vim_keybindings.normal]
            end_of_line = ";"
            "#,
        )
        .unwrap();

        use crate::keymap::{KeyChord, KeyCode, KeyModifiers, VimAction};

        // Old binding ($) should not invoke EndOfLine
        let dollar = KeyChord {
            code: KeyCode::Char('$'),
            modifiers: KeyModifiers::default(),
        };
        assert_ne!(
            config.keymap.vim.normal.get(&dollar),
            Some(&VimAction::EndOfLine)
        );

        // New binding (;) should invoke EndOfLine
        let semicolon = KeyChord {
            code: KeyCode::Char(';'),
            modifiers: KeyModifiers::default(),
        };
        assert_eq!(
            config.keymap.vim.normal.get(&semicolon),
            Some(&VimAction::EndOfLine)
        );

        // Other bindings should be intact
        let h = KeyChord {
            code: KeyCode::Char('h'),
            modifiers: KeyModifiers::default(),
        };
        assert_eq!(
            config.keymap.vim.normal.get(&h),
            Some(&VimAction::CursorLeft)
        );
    }

    #[test]
    fn vim_keybinding_override_insert() {
        let config = parse(
            r#"
            [vim_keybindings.insert]
            exit_insert_mode = "ctrl-j"
            "#,
        )
        .unwrap();

        use crate::keymap::{KeyChord, KeyCode, KeyModifiers, VimAction};

        // Default Esc binding should be gone
        let esc = KeyChord {
            code: KeyCode::Esc,
            modifiers: KeyModifiers::default(),
        };
        assert_ne!(
            config.keymap.vim.insert.get(&esc),
            Some(&VimAction::ExitInsertMode)
        );

        // New binding (ctrl-j) should be present
        let ctrl_j = KeyChord {
            code: KeyCode::Char('j'),
            modifiers: KeyModifiers {
                ctrl: true,
                alt: false,
            },
        };
        assert_eq!(
            config.keymap.vim.insert.get(&ctrl_j),
            Some(&VimAction::ExitInsertMode)
        );
    }

    #[test]
    fn gg_binding_can_be_overridden() {
        let config = parse(
            r#"
            [keybindings]
            scroll_top = "ctrl-t"
            "#,
        )
        .unwrap();

        // gg should no longer be bound
        assert_eq!(config.keymap.gg_action, None);

        // ctrl-t should be ScrollTop
        use crate::keymap::{GlobalAction, KeyChord, KeyCode, KeyModifiers};
        let ctrl_t = KeyChord {
            code: KeyCode::Char('t'),
            modifiers: KeyModifiers {
                ctrl: true,
                alt: false,
            },
        };
        assert_eq!(
            config.keymap.globals.get(&ctrl_t),
            Some(&GlobalAction::ScrollTop)
        );
    }

    #[test]
    fn config_round_trips_vim_mode() {
        // Full example config from §28
        let toml_str = r#"
input_mode = "vim"
shell = "zsh"
debounce_ms = 100
clipboard_command = "pbcopy"

[keybindings]
copy_output = "ctrl-y"
copy_command = "alt-y"
toggle_error_pane = "ctrl-e"
scroll_half_page_down = "ctrl-d"
scroll_half_page_up = "ctrl-u"
scroll_top = "gg"
scroll_bottom = "G"

[environment]
RG_WRAPPER_DISABLE = "1"

[vim_keybindings.normal]
end_of_line = ";"
"#;
        let config = parse(toml_str).unwrap();
        assert_eq!(config.input_mode, InputMode::Vim);
        assert_eq!(config.shell, "zsh");
        assert_eq!(config.debounce_ms, 100);
        assert_eq!(config.clipboard_command, "pbcopy");
        assert_eq!(config.environment.get("RG_WRAPPER_DISABLE").unwrap(), "1");
        // gg should still be ScrollTop (since we set it explicitly)
        assert_eq!(
            config.keymap.gg_action,
            Some(crate::keymap::GlobalAction::ScrollTop)
        );
    }
}
