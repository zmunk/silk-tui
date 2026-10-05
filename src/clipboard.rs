//! Replaceable clipboard abstraction (§26–27).
//!
//! Clipboard operations are deliberately independent of the renderer. Callers can turn a
//! successful operation into a short-lived `COPIED_MESSAGE`, and display any returned error.

use crate::editor::EditorState;
use crate::output::OutputState;
use anyhow::{Context, Result, bail};
use std::io::Write;
use std::process::{Command, Stdio};

/// Message shown by the app after a successful copy.
pub const COPIED_MESSAGE: &str = "copied";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clipboard {
    /// External command which receives clipboard contents on standard input.
    pub command: String,
}

impl Clipboard {
    pub fn new(command: impl Into<String>) -> Self {
        Self {
            command: command.into(),
        }
    }

    /// Pipe `text` to the configured clipboard command.
    ///
    /// The command may contain arguments (for example, `xclip -selection clipboard`).
    /// A successful spawn is not sufficient: a non-zero command exit status is reported as
    /// an error so the app can show a useful temporary failure message.
    pub fn copy(&self, text: &str) -> Result<()> {
        if self.command.trim().is_empty() {
            bail!("clipboard command is empty");
        }

        let mut command = platform_command(&self.command);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to start clipboard command `{}`", self.command))?;

        // Always wait for the child, even if it closes stdin before all text is written.
        let write_result = child
            .stdin
            .take()
            .context("clipboard command stdin was unavailable")
            .and_then(|mut stdin| {
                stdin
                    .write_all(text.as_bytes())
                    .context("failed to write to clipboard command")
            });

        let output = child
            .wait_with_output()
            .context("failed to wait for clipboard command")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            if stderr.is_empty() {
                bail!(
                    "clipboard command `{}` failed with status {}",
                    self.command,
                    output.status
                );
            }
            bail!(
                "clipboard command `{}` failed with status {}: {}",
                self.command,
                output.status,
                stderr
            );
        }

        write_result?;
        Ok(())
    }

    /// Copy the last successful stdout. Returns `false` when no successful output exists yet.
    /// Empty stdout from a successful command is still copied.
    pub fn copy_output(&self, state: &OutputState) -> Result<bool> {
        let Some(result) = state.last_success.as_ref() else {
            return Ok(false);
        };

        self.copy(&result.stdout)?;
        Ok(true)
    }

    /// Copy the editor's complete current text.
    pub fn copy_command(&self, editor: &EditorState) -> Result<()> {
        self.copy(&editor.text())
    }
}

#[cfg(not(target_os = "windows"))]
fn platform_command(command: &str) -> Command {
    let mut process = Command::new("sh");
    process.arg("-c").arg(command);
    process
}

#[cfg(target_os = "windows")]
fn platform_command(command: &str) -> Command {
    let mut process = Command::new("cmd.exe");
    process.arg("/C").arg(command);
    process
}

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::*;
    use crate::output::{EvaluationKind, EvaluationResult};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_FILE: AtomicU64 = AtomicU64::new(0);

    fn destination() -> PathBuf {
        let id = NEXT_FILE.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("silk-clipboard-{}-{id}", std::process::id()))
    }

    fn command_for(path: &std::path::Path) -> String {
        format!("cat > '{}'", path.display())
    }

    fn successful_output(stdout: &str) -> OutputState {
        let mut state = OutputState::new();
        state.last_success = Some(EvaluationResult {
            generation: 1,
            command: "printf output".into(),
            stdout: stdout.into(),
            stderr: String::new(),
            exit_code: Some(0),
            kind: EvaluationKind::Success,
        });
        state
    }

    #[test]
    fn copy_pipes_exact_text_to_command_stdin() {
        let path = destination();
        let clipboard = Clipboard::new(command_for(&path));

        clipboard.copy("first\nsecond").unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "first\nsecond");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn copy_output_uses_last_success() {
        let path = destination();
        let clipboard = Clipboard::new(command_for(&path));
        let state = successful_output("preview output");

        assert!(clipboard.copy_output(&state).unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "preview output");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn copy_output_without_success_is_a_noop() {
        let path = destination();
        let clipboard = Clipboard::new(command_for(&path));

        assert!(!clipboard.copy_output(&OutputState::new()).unwrap());
        assert!(!path.exists());
    }

    #[test]
    fn copy_command_uses_complete_editor_text() {
        let path = destination();
        let clipboard = Clipboard::new(command_for(&path));
        let mut editor = EditorState::new();
        editor.set_text("echo one\necho two");

        clipboard.copy_command(&editor).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "echo one\necho two");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn nonzero_exit_is_reported_with_stderr() {
        let clipboard = Clipboard::new("echo unavailable >&2; exit 7");

        let error = clipboard.copy("text").unwrap_err().to_string();

        assert!(error.contains("unavailable"));
        assert!(error.contains("status"));
    }

    #[test]
    fn empty_command_is_rejected() {
        let error = Clipboard::new("  ").copy("text").unwrap_err().to_string();
        assert!(error.contains("empty"));
    }
}
