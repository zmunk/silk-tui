//! Fullscreen TUI transport. Process stdout is exclusively the shell protocol.

use crossterm::cursor::Show;
use crossterm::event::DisableBracketedPaste;
use crossterm::terminal::LeaveAlternateScreen;
use crossterm::{cursor::SetCursorStyle, execute};
use ratatui::backend::{Backend, ClearType, CrosstermBackend, WindowSize};
use ratatui::buffer::Cell;
use ratatui::layout::{Position, Size};
#[cfg(unix)]
use std::fs::{File, OpenOptions};
use std::io;

#[cfg(unix)]
pub(crate) type TtyOutput = File;
#[cfg(not(unix))]
pub(crate) type TtyOutput = io::Stderr;

pub(crate) fn clone_tty(output: &TtyOutput) -> io::Result<TtyOutput> {
    #[cfg(unix)]
    {
        output.try_clone()
    }
    #[cfg(not(unix))]
    {
        let _ = output;
        Ok(io::stderr())
    }
}

pub(crate) fn open_tty() -> io::Result<TtyOutput> {
    #[cfg(unix)]
    {
        OpenOptions::new().read(true).write(true).open("/dev/tty")
    }
    #[cfg(not(unix))]
    {
        Ok(io::stderr())
    }
}

/// Attempt every restoration step even if an earlier write failed.
pub(crate) fn restore_output(output: &mut TtyOutput) -> io::Result<()> {
    let style = execute!(output, SetCursorStyle::DefaultUserShape);
    let paste = execute!(output, DisableBracketedPaste);
    let screen = execute!(output, LeaveAlternateScreen);
    let cursor = execute!(output, Show);
    style.and(paste).and(screen).and(cursor)
}

/// Crossterm's cursor::position() writes its query to process stdout, not the
/// backend writer. A fullscreen alternate screen never needs that query: its
/// initial cursor position is (0, 0), and Silk explicitly positions each frame.
/// Size queries also use our owned TTY rather than stdout/stderr fallbacks.
pub(crate) struct TtyBackend {
    inner: CrosstermBackend<TtyOutput>,
    output: TtyOutput,
    cursor: Position,
}

impl TtyBackend {
    pub(crate) fn new(output: TtyOutput, rendering: TtyOutput) -> Self {
        Self {
            inner: CrosstermBackend::new(rendering),
            output,
            cursor: Position::new(0, 0),
        }
    }

    pub(crate) fn output(&mut self) -> &mut TtyOutput {
        &mut self.output
    }

    fn tty_size(&self) -> io::Result<WindowSize> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let mut size: libc::winsize = unsafe { std::mem::zeroed() };
            // SAFETY: the descriptor remains owned by this backend and `size`
            // is a valid writable winsize for the duration of the ioctl.
            if unsafe { libc::ioctl(self.output.as_raw_fd(), libc::TIOCGWINSZ, &mut size) } == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(WindowSize {
                columns_rows: Size::new(size.ws_col, size.ws_row),
                pixels: Size::new(size.ws_xpixel, size.ws_ypixel),
            })
        }
        #[cfg(not(unix))]
        {
            let size = crossterm::terminal::window_size()?;
            Ok(WindowSize {
                columns_rows: Size::new(size.columns, size.rows),
                pixels: Size::new(size.width, size.height),
            })
        }
    }
}

impl Backend for TtyBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        self.inner.hide_cursor()
    }
    fn show_cursor(&mut self) -> io::Result<()> {
        self.inner.show_cursor()
    }
    fn get_cursor_position(&mut self) -> io::Result<Position> {
        Ok(self.cursor)
    }
    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
        let position = position.into();
        self.inner.set_cursor_position(position)?;
        self.cursor = position;
        Ok(())
    }
    fn clear(&mut self) -> io::Result<()> {
        self.inner.clear()
    }
    fn clear_region(&mut self, clear_type: ClearType) -> io::Result<()> {
        self.inner.clear_region(clear_type)
    }
    fn append_lines(&mut self, n: u16) -> io::Result<()> {
        self.inner.append_lines(n)
    }
    fn size(&self) -> io::Result<Size> {
        Ok(self.tty_size()?.columns_rows)
    }
    fn window_size(&mut self) -> io::Result<WindowSize> {
        self.tty_size()
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
