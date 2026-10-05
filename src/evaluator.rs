//! Non-blocking shell syntax checking and command execution (§7–9).

use crate::output::{EvaluationKind, EvaluationResult};
use std::collections::HashMap;
#[cfg(unix)]
use std::fs::File;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::fd::FromRawFd;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc::Sender};
use std::thread;
use std::time::Duration;

struct RunningChild {
    generation: u64,
    child: Child,
}

/// Executes preview commands on worker threads.
///
/// `Command` inherits the parent environment; `env_vars` are applied on top.
pub struct Evaluator {
    pub shell: String,
    pub env_vars: HashMap<String, String>,
    generation: Arc<AtomicU64>,
    current_child: Arc<Mutex<Option<RunningChild>>>,
}

impl Evaluator {
    pub fn new(shell: impl Into<String>, env_vars: HashMap<String, String>) -> Self {
        Self {
            shell: shell.into(),
            env_vars,
            generation: Arc::new(AtomicU64::new(0)),
            current_child: Arc::new(Mutex::new(None)),
        }
    }

    /// Allocate the next monotonically increasing generation ID.
    pub fn next_generation(&self) -> u64 {
        self.generation.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// Syntax-check and evaluate `command` without blocking the caller.
    ///
    /// Starting a newer generation cancels the currently running shell. A
    /// cancelled or otherwise superseded worker does not send a result.
    pub fn evaluate(
        &self,
        command: &str,
        generation: u64,
        tx: Sender<EvaluationResult>,
        preview_size: Option<(u16, u16)>,
    ) {
        self.generation.fetch_max(generation, Ordering::SeqCst);
        cancel_older_child(&self.current_child, generation);

        let shell = self.shell.clone();
        let env_vars = self.env_vars.clone();
        let command = command.to_owned();
        let latest = Arc::clone(&self.generation);
        let current_child = Arc::clone(&self.current_child);

        thread::spawn(move || {
            if latest.load(Ordering::SeqCst) != generation {
                return;
            }

            if command.trim().is_empty() {
                let _ = tx.send(EvaluationResult {
                    generation,
                    command,
                    stdout: String::new(),
                    stderr: String::new(),
                    exit_code: Some(0),
                    kind: EvaluationKind::Empty,
                });
                return;
            }

            let mut syntax_command = Command::new(&shell);
            syntax_command
                .args(["-n", "-c", &command])
                .envs(&env_vars)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            apply_preview_size(&mut syntax_command, preview_size);
            let syntax = syntax_command.spawn();

            let syntax = match syntax {
                Ok(child) => run_child(child, generation, &latest, &current_child, None),
                Err(error) => {
                    send_spawn_error(&tx, generation, command, error);
                    return;
                }
            };

            let syntax = match syntax {
                Some(output) => output,
                None => return,
            };

            if !syntax.status.success() {
                let _ = tx.send(EvaluationResult {
                    generation,
                    command,
                    stdout: syntax.stdout,
                    stderr: syntax.stderr,
                    exit_code: syntax.status.code(),
                    kind: EvaluationKind::SyntaxError,
                });
                return;
            }

            if latest.load(Ordering::SeqCst) != generation {
                return;
            }

            let script = execution_script(&shell);
            let mut execution_command = Command::new(&shell);
            execution_command
                .args(["-c", script, "silk", &command])
                .envs(&env_vars)
                .stderr(Stdio::piped());
            apply_preview_size(&mut execution_command, preview_size);
            let preview_reader =
                match configure_preview_output(&mut execution_command, preview_size) {
                    Ok(reader) => reader,
                    Err(error) => {
                        send_spawn_error(&tx, generation, command, error);
                        return;
                    }
                };
            let execution = execution_command.spawn();
            // `Command` retains its configured PTY slave. Close that parent-side
            // descriptor so the master reader observes EOF after the child exits.
            drop(execution_command);

            let execution = match execution {
                Ok(child) => run_child(child, generation, &latest, &current_child, preview_reader),
                Err(error) => {
                    send_spawn_error(&tx, generation, command, error);
                    return;
                }
            };

            let Some(output) = execution else {
                return;
            };
            if latest.load(Ordering::SeqCst) != generation {
                return;
            }

            let kind = if execution_succeeded(&output.status) {
                EvaluationKind::Success
            } else {
                EvaluationKind::Failure
            };
            let _ = tx.send(EvaluationResult {
                generation,
                command,
                stdout: output.stdout,
                stderr: output.stderr,
                exit_code: output.status.code(),
                kind,
            });
        });
    }

    /// Terminate the currently running syntax check or preview, if any.
    pub fn cancel(&self) {
        // Invalidate workers that may currently be between syntax checking and
        // execution, as well as terminating a child that is already running.
        self.generation.fetch_add(1, Ordering::SeqCst);
        if let Ok(mut slot) = self.current_child.lock() {
            if let Some(mut running) = slot.take() {
                let _ = running.child.kill();
                let _ = running.child.wait();
            }
        }
    }
}

impl Drop for Evaluator {
    fn drop(&mut self) {
        self.cancel();
    }
}

struct CapturedOutput {
    status: ExitStatus,
    stdout: String,
    stderr: String,
}

/// Drain both pipes concurrently to avoid deadlocking on a full OS pipe while
/// retaining a cancellable handle to the child.
fn run_child(
    mut child: Child,
    generation: u64,
    latest: &AtomicU64,
    current: &Mutex<Option<RunningChild>>,
    preview_reader: Option<Box<dyn Read + Send>>,
) -> Option<CapturedOutput> {
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let stdout_reader = thread::spawn(move || match preview_reader {
        Some(reader) => read_pipe(Some(reader)).replace("\r\n", "\n"),
        None => read_pipe(stdout),
    });
    let stderr_reader = thread::spawn(move || read_pipe(stderr));

    {
        let mut slot = current.lock().ok()?;
        if latest.load(Ordering::SeqCst) != generation {
            let _ = child.kill();
            let _ = child.wait();
            drop(slot);
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return None;
        }
        if let Some(mut old) = slot.take() {
            let _ = old.child.kill();
            let _ = old.child.wait();
        }
        *slot = Some(RunningChild { generation, child });
    }

    let status = loop {
        let mut slot = current.lock().ok()?;
        let Some(running) = slot.as_mut() else {
            drop(slot);
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return None;
        };
        if running.generation != generation || latest.load(Ordering::SeqCst) != generation {
            if running.generation == generation {
                let mut running = slot.take().expect("checked above");
                let _ = running.child.kill();
                let _ = running.child.wait();
            }
            drop(slot);
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            return None;
        }
        match running.child.try_wait() {
            Ok(Some(status)) => {
                slot.take();
                break status;
            }
            Ok(None) => {}
            Err(_) => {
                let mut running = slot.take().expect("checked above");
                let _ = running.child.kill();
                let _ = running.child.wait();
                drop(slot);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return None;
            }
        }
        drop(slot);
        thread::sleep(Duration::from_millis(10));
    };

    Some(CapturedOutput {
        status,
        stdout: stdout_reader.join().unwrap_or_default(),
        stderr: stderr_reader.join().unwrap_or_default(),
    })
}

#[cfg(unix)]
fn configure_preview_output(
    command: &mut Command,
    preview_size: Option<(u16, u16)>,
) -> io::Result<Option<Box<dyn Read + Send>>> {
    let Some((columns, lines)) = preview_size else {
        command.stdout(Stdio::piped());
        return Ok(None);
    };

    let size = libc::winsize {
        ws_row: lines,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let mut master = -1;
    let mut slave = -1;
    // SAFETY: openpty initializes both file descriptors; null termios means
    // platform defaults, and `size` remains valid for the duration of the call.
    if unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &size,
        )
    } == -1
    {
        return Err(io::Error::last_os_error());
    }

    // SAFETY: openpty returned two newly owned descriptors on success.
    let master = unsafe { File::from_raw_fd(master) };
    let slave = unsafe { File::from_raw_fd(slave) };
    command.stdout(Stdio::from(slave));
    Ok(Some(Box::new(master)))
}

#[cfg(not(unix))]
fn configure_preview_output(
    command: &mut Command,
    _preview_size: Option<(u16, u16)>,
) -> io::Result<Option<Box<dyn Read + Send>>> {
    command.stdout(Stdio::piped());
    Ok(None)
}

fn apply_preview_size(command: &mut Command, preview_size: Option<(u16, u16)>) {
    if let Some((columns, lines)) = preview_size {
        command.env("COLUMNS", columns.to_string());
        command.env("LINES", lines.to_string());
    }
}

fn read_pipe<R: Read>(pipe: Option<R>) -> String {
    let mut bytes = Vec::new();
    if let Some(mut pipe) = pipe {
        let _ = pipe.read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

fn cancel_older_child(current: &Mutex<Option<RunningChild>>, generation: u64) {
    if let Ok(mut slot) = current.lock() {
        if slot
            .as_ref()
            .is_some_and(|child| child.generation < generation)
        {
            let mut running = slot.take().expect("checked above");
            let _ = running.child.kill();
            let _ = running.child.wait();
        }
    }
}

/// With `pipefail`, an upstream process terminated by SIGPIPE makes an otherwise
/// successful short-consuming pipeline (for example, `producer | take 1`) exit 141.
/// Treat that expected early-close status as success while preserving other failures.
fn execution_succeeded(status: &ExitStatus) -> bool {
    status.success() || status.code() == Some(128 + 13)
}

fn execution_script(shell: &str) -> &'static str {
    if std::path::Path::new(shell)
        .file_name()
        .is_some_and(|name| name.to_string_lossy().contains("zsh"))
    {
        "setopt pipefail; eval \"$1\""
    } else {
        "set -o pipefail; eval \"$1\""
    }
}

fn send_spawn_error(
    tx: &Sender<EvaluationResult>,
    generation: u64,
    command: String,
    error: std::io::Error,
) {
    let _ = tx.send(EvaluationResult {
        generation,
        command,
        stdout: String::new(),
        stderr: error.to_string(),
        exit_code: None,
        kind: EvaluationKind::SpawnError,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn evaluate(command: &str) -> EvaluationResult {
        let evaluator = Evaluator::new("bash", HashMap::new());
        let generation = evaluator.next_generation();
        let (tx, rx) = mpsc::channel();
        evaluator.evaluate(command, generation, tx, None);
        rx.recv_timeout(Duration::from_secs(5)).unwrap()
    }

    #[test]
    fn success_captures_stdout() {
        let result = evaluate("printf hello");
        assert_eq!(result.kind, EvaluationKind::Success);
        assert_eq!(result.stdout, "hello");
        assert!(result.stderr.is_empty());
        assert_eq!(result.exit_code, Some(0));
    }

    #[test]
    fn sigpipe_from_short_consuming_pipeline_is_success() {
        let result = evaluate("yes | head -n 1");
        assert_eq!(result.kind, EvaluationKind::Success);
        assert_eq!(result.stdout, "y\n");
        assert_eq!(result.exit_code, Some(141));
    }

    #[test]
    fn failure_captures_stderr() {
        let result = evaluate("printf failure >&2; exit 7");
        assert_eq!(result.kind, EvaluationKind::Failure);
        assert_eq!(result.stderr, "failure");
        assert_eq!(result.exit_code, Some(7));
    }

    #[test]
    fn syntax_error_is_not_executed() {
        let marker = std::env::temp_dir().join(format!(
            "silk-syntax-test-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("worker")
        ));
        let command = format!("touch {}; if", marker.display());
        let result = evaluate(&command);
        assert_eq!(result.kind, EvaluationKind::SyntaxError);
        assert!(!marker.exists());
    }

    #[test]
    fn successful_command_retains_stderr() {
        let result = evaluate("printf warning >&2; printf output");
        assert_eq!(result.kind, EvaluationKind::Success);
        assert_eq!(result.stdout, "output");
        assert_eq!(result.stderr, "warning");
    }

    #[test]
    fn preview_size_is_exposed_to_commands() {
        let evaluator = Evaluator::new("bash", HashMap::new());
        let generation = evaluator.next_generation();
        let (tx, rx) = mpsc::channel();
        evaluator.evaluate(
            "printf '%s x %s' \"$COLUMNS\" \"$LINES\"",
            generation,
            tx,
            Some((80, 24)),
        );
        let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(result.stdout, "80 x 24");
    }

    #[cfg(unix)]
    #[test]
    fn preview_pty_reports_pane_dimensions() {
        let evaluator = Evaluator::new("bash", HashMap::new());
        let generation = evaluator.next_generation();
        let (tx, rx) = mpsc::channel();
        // `stty` inspects stdin by default; redirect it from preview stdout so
        // this verifies the pane-sized PTY rather than the parent's terminal.
        evaluator.evaluate("stty size <&1", generation, tx, Some((80, 24)));
        let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(result.stdout, "24 80\n");
    }

    #[test]
    fn configured_environment_overrides_parent() {
        let mut environment = HashMap::new();
        environment.insert("SILK_EVALUATOR_TEST".into(), "configured".into());
        let evaluator = Evaluator::new("bash", environment);
        let generation = evaluator.next_generation();
        let (tx, rx) = mpsc::channel();
        evaluator.evaluate("printf %s \"$SILK_EVALUATOR_TEST\"", generation, tx, None);
        let result = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(result.stdout, "configured");
    }
}
