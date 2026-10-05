//! Output state: last_success / current_attempt tracking, evaluation status, and scrolling.

/// The kind of evaluation result (§5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationKind {
    Empty,
    Success,
    Failure,
    SyntaxError,
    SpawnError,
}

/// The result of a single command evaluation (§5).
#[derive(Debug, Clone)]
pub struct EvaluationResult {
    pub generation: u64,
    pub command: String,
    pub stdout: String,
    pub stderr: String,
    pub exit_code: Option<i32>,
    pub kind: EvaluationKind,
}

/// The current evaluation status (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvaluationStatus {
    /// Current command is empty.
    Empty,
    /// Current command is being evaluated.
    Running,
    /// Displayed stdout belongs to the current command.
    Current,
    /// Displayed stdout belongs to an earlier successful command.
    Stale,
}

/// Tracks last-success and current-attempt outputs, status, and scroll offsets (§4, §10, §14).
pub struct OutputState {
    pub last_success: Option<EvaluationResult>,
    pub current_attempt: Option<EvaluationResult>,
    pub status: EvaluationStatus,
    pub output_scroll: usize,
    pub error_scroll: usize,
    pub error_pane_visible: bool,
}

impl OutputState {
    pub fn new() -> Self {
        Self {
            last_success: None,
            current_attempt: None,
            status: EvaluationStatus::Empty,
            output_scroll: 0,
            error_scroll: 0,
            error_pane_visible: false,
        }
    }

    /// Apply an evaluation result, respecting generation ordering and §10 semantics.
    /// Returns true if the result was applied (not stale).
    pub fn apply_result(&mut self, result: EvaluationResult, current_gen: u64) -> bool {
        // Ignore stale generations (§9).
        if result.generation < current_gen {
            return false;
        }

        match result.kind {
            EvaluationKind::Success | EvaluationKind::Empty => {
                // §10: On success: last_success = current result,
                //      current_attempt = current result, status = Current.
                self.last_success = Some(result.clone());
                self.current_attempt = Some(result);
                self.status = EvaluationStatus::Current;
            }
            EvaluationKind::Failure
            | EvaluationKind::SyntaxError
            | EvaluationKind::SpawnError => {
                // §10: On failure: last_success = unchanged,
                //      current_attempt = failed result, status = Stale.
                self.current_attempt = Some(result);
                self.status = EvaluationStatus::Stale;
            }
        }
        true
    }

    /// Mark evaluation as started.
    pub fn mark_running(&mut self) {
        self.status = EvaluationStatus::Running;
    }

    /// Mark evaluation as empty (command cleared).
    pub fn mark_empty(&mut self) {
        self.current_attempt = None;
        self.status = EvaluationStatus::Empty;
    }

    /// Determine which pane scrolling targets (§14).
    /// Returns true if scrolling should operate on the error pane.
    pub fn scroll_target_is_error(&self) -> bool {
        self.error_pane_visible
            && self
                .current_attempt
                .as_ref()
                .map_or(false, |r| !r.stderr.is_empty())
    }

    // --- Scroll helpers ---

    pub fn scroll_output_half_page_down(&mut self, page_height: usize) {
        self.output_scroll = self.output_scroll.saturating_add(page_height / 2);
    }

    pub fn scroll_output_half_page_up(&mut self, page_height: usize) {
        self.output_scroll = self.output_scroll.saturating_sub(page_height / 2);
    }

    pub fn scroll_output_top(&mut self) {
        self.output_scroll = 0;
    }

    pub fn scroll_output_bottom(&mut self, max: usize) {
        self.output_scroll = max;
    }

    pub fn scroll_error_half_page_down(&mut self, page_height: usize) {
        self.error_scroll = self.error_scroll.saturating_add(page_height / 2);
    }

    pub fn scroll_error_half_page_up(&mut self, page_height: usize) {
        self.error_scroll = self.error_scroll.saturating_sub(page_height / 2);
    }

    pub fn scroll_error_top(&mut self) {
        self.error_scroll = 0;
    }

    pub fn scroll_error_bottom(&mut self, max: usize) {
        self.error_scroll = max;
    }
}
