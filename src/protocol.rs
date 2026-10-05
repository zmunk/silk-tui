//! Shell-facing argument, output, and terminal-restoration protocol.

use anyhow::{Context, bail};
use crossterm::terminal::disable_raw_mode;
use crate::terminal::TtyOutput;
use std::ffi::OsString;
use std::io::{self, Write};
use std::sync::{Mutex, Once};
use std::sync::atomic::{AtomicBool, Ordering};

static TERMINAL_ACTIVE: AtomicBool = AtomicBool::new(false);
static PANIC_HOOK: Once = Once::new();
static RESTORE_TTY: Mutex<Option<TtyOutput>> = Mutex::new(None);

/// Parse the optional `--query "$BUFFER"` argument. Defaults to an empty buffer
/// when omitted, so Silk can be launched without a pre-filled command.
pub fn parse_args() -> anyhow::Result<String> {
    parse_args_from(std::env::args_os().skip(1))
}

fn parse_args_from(args: impl IntoIterator<Item = OsString>) -> anyhow::Result<String> {
    let mut args = args.into_iter();
    let mut query = None;

    while let Some(argument) = args.next() {
        if argument == "--query" {
            if query.is_some() {
                bail!("--query may only be specified once");
            }
            let value = args.next().context("--query requires a value")?;
            query = Some(
                value
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("--query must be valid UTF-8"))?,
            );
        } else if let Some(value) = argument
            .to_str()
            .and_then(|arg| arg.strip_prefix("--query="))
        {
            if query.replace(value.to_owned()).is_some() {
                bail!("--query may only be specified once");
            }
        } else {
            bail!("unexpected argument `{}`", argument.to_string_lossy());
        }
    }

    Ok(query.unwrap_or_default())
}

/// Emit the shell-returned command exactly, with no diagnostic text or added newline.
pub fn write_command(mut output: impl Write, command: Option<&str>) -> io::Result<()> {
    if let Some(command) = command {
        output.write_all(command.as_bytes())?;
    }
    output.flush()
}

/// Install one process-wide hook which restores the terminal before panic output.
pub fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let main_thread = std::thread::current().id();
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // A preview worker panic must not tear down a still-running UI.
            if std::thread::current().id() == main_thread {
                emergency_restore_terminal();
            }
            previous(info);
        }));
    });
}

pub(crate) fn terminal_started(tty: TtyOutput) {
    *RESTORE_TTY.lock().unwrap_or_else(|error| error.into_inner()) = Some(tty);
    TERMINAL_ACTIVE.store(true, Ordering::SeqCst);
}

pub(crate) fn terminal_restored() {
    TERMINAL_ACTIVE.store(false, Ordering::SeqCst);
    RESTORE_TTY.lock().unwrap_or_else(|error| error.into_inner()).take();
}

pub(crate) fn emergency_restore_terminal() {
    if TERMINAL_ACTIVE.swap(false, Ordering::SeqCst) {
        let _ = disable_raw_mode();
        if let Some(mut tty) = RESTORE_TTY.lock().unwrap_or_else(|error| error.into_inner()).take() {
            let _ = crate::terminal::restore_output(&mut tty);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> anyhow::Result<String> {
        parse_args_from(args.iter().map(|argument| OsString::from(*argument)))
    }

    #[test]
    fn parses_query_in_both_supported_forms() {
        assert_eq!(parse(&["--query", "echo hello"]).unwrap(), "echo hello");
        assert_eq!(parse(&["--query=printf ok"]).unwrap(), "printf ok");
        assert_eq!(parse(&["--query", ""]).unwrap(), "");
    }

    #[test]
    fn rejects_missing_duplicate_and_unknown_arguments() {
        assert_eq!(parse(&[]).unwrap(), "");
        assert!(parse(&["--query"]).is_err());
        assert!(parse(&["--query", "a", "--query", "b"]).is_err());
        assert!(parse(&["--other", "a"]).is_err());
    }

    #[test]
    fn stdout_contract_is_exact() {
        let mut output = Vec::new();
        write_command(&mut output, Some("echo one\necho two")).unwrap();
        assert_eq!(output, b"echo one\necho two");

        output.clear();
        write_command(&mut output, None).unwrap();
        assert!(output.is_empty());
    }
}
