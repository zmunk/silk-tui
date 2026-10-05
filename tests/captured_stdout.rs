#![cfg(unix)]

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Running(Child);
impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn termios(tty: &File) -> libc::termios {
    let mut state = unsafe { std::mem::zeroed() };
    assert_eq!(unsafe { libc::tcgetattr(tty.as_raw_fd(), &mut state) }, 0);
    state
}

fn captured_session(keys: &[u8], code: i32, command: Option<&str>, invalid_args: bool) {
    let mut master_fd = -1;
    let mut slave_fd = -1;
    let size = libc::winsize { ws_row: 24, ws_col: 80, ws_xpixel: 0, ws_ypixel: 0 };
    assert_eq!(unsafe {
        libc::openpty(&mut master_fd, &mut slave_fd, std::ptr::null_mut(), std::ptr::null(), &size)
    }, 0);
    let mut master = unsafe { File::from_raw_fd(master_fd) };
    let slave = unsafe { File::from_raw_fd(slave_fd) };
    let original = termios(&slave);
    assert_ne!(unsafe { libc::fcntl(master_fd, libc::F_SETFD, libc::FD_CLOEXEC) }, -1);
    assert_ne!(unsafe { libc::fcntl(slave_fd, libc::F_SETFD, libc::FD_CLOEXEC) }, -1);
    assert_ne!(unsafe { libc::fcntl(master_fd, libc::F_SETFL, libc::O_NONBLOCK) }, -1);

    let mut process = Command::new(env!("CARGO_BIN_EXE_silk"));
    process
        .args(if invalid_args { vec!["--invalid"] } else { vec!["--query", "printf silk-test"] })
        // Isolate tests from user configuration; no config is read under /dev/null.
        .env("XDG_CONFIG_HOME", "/dev/null")
        .env("TERM", "xterm-256color")
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    unsafe {
        process.pre_exec(|| {
            if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = Running(process.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut screen = Vec::new();
    let mut sent = invalid_args;
    let status = loop {
        let mut bytes = [0; 8192];
        match master.read(&mut bytes) {
            Ok(n) => screen.extend_from_slice(&bytes[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("PTY read failed: {error}"),
        }
        // A cursor-style update occurs after the first complete frame. No
        // cursor report should ever be requested (and thus none can enter ZLE).
        assert!(!screen.windows(4).any(|bytes| bytes == b"\x1b[6n"), "unexpected cursor query");
        if !sent && screen.windows(2).any(|bytes| bytes == b" q") {
            master.write_all(keys).unwrap();
            sent = true;
        }
        if let Some(status) = child.0.try_wait().unwrap() { break status; }
        assert!(Instant::now() < deadline, "Silk did not render/exit: {:?}", String::from_utf8_lossy(&screen));
        std::thread::sleep(Duration::from_millis(10));
    };
    let mut stdout = Vec::new();
    child.0.stdout.take().unwrap().read_to_end(&mut stdout).unwrap();
    let mut stderr = String::new();
    child.0.stderr.take().unwrap().read_to_string(&mut stderr).unwrap();
    assert_eq!(status.code(), Some(code), "{stderr}");
    assert_eq!(stdout, command.unwrap_or("").as_bytes(), "protocol stdout polluted");
    assert!(!stdout.contains(&0x1b));
    if invalid_args {
        assert!(stderr.contains("unexpected argument"), "diagnostics were lost: {stderr}");
    } else {
        assert!(stderr.is_empty(), "TUI or diagnostics unexpectedly on stderr: {stderr}");
        // Collect final teardown bytes written just before process exit.
        let mut bytes = [0; 8192];
        while let Ok(n) = master.read(&mut bytes) {
            if n == 0 { break; }
            screen.extend_from_slice(&bytes[..n]);
        }
        assert!(screen.windows(8).any(|bytes| bytes == b"\x1b[?1049l"), "alternate screen not restored");
        assert!(screen.windows(6).any(|bytes| bytes == b"\x1b[?25h"), "cursor not restored");
        assert!(!screen.windows(4).any(|bytes| bytes == b"\x1b[6n"));
    }
    let restored = termios(&slave);
    assert_eq!(restored.c_iflag, original.c_iflag);
    assert_eq!(restored.c_oflag, original.c_oflag);
    assert_eq!(restored.c_cflag, original.c_cflag);
    assert_eq!(restored.c_lflag, original.c_lflag);
    assert_eq!(restored.c_cc, original.c_cc);
    // Probe the restored canonical input queue, as the shell would. Only our
    // newline may be present; any leftover cursor response would precede it.
    master.write_all(b"\n").unwrap();
    let mut ready = libc::pollfd { fd: slave_fd, events: libc::POLLIN, revents: 0 };
    assert_eq!(unsafe { libc::poll(&mut ready, 1, 1000) }, 1);
    let mut remaining = [0; 128];
    let n = (&slave).read(&mut remaining).unwrap();
    assert_eq!(&remaining[..n], b"\n", "terminal input leaked back to the shell");
}

#[test]
fn keep_with_stdout_and_stderr_captured() {
    captured_session(b"\x11", 0, Some("printf silk-test"), false);
}

#[test]
fn execute_with_stdout_and_stderr_captured() {
    captured_session(b"\r", 10, Some("printf silk-test"), false);
}

#[test]
fn cancel_with_stdout_and_stderr_captured() {
    captured_session(b"\x03\x03", 130, None, false);
}

#[test]
fn unexpected_failure_leaves_protocol_and_terminal_untouched() {
    captured_session(b"", 1, None, true);
}
