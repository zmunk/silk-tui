//! Application controller — owns state, wires components together, runs the event loop.

use crate::editor::EditorState;
use crate::output::OutputState;

/// Top-level application state (§4).
pub struct AppState {
    pub editor: EditorState,
    pub output: OutputState,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            editor: EditorState::new(),
            output: OutputState::new(),
        }
    }
}

pub struct App {
    pub state: AppState,
}

impl App {
    pub fn new() -> Self {
        Self {
            state: AppState::new(),
        }
    }

    pub fn run(&mut self) -> anyhow::Result<()> {
        // TODO: event loop
        Ok(())
    }
}
