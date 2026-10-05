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
            EvaluationKind::Success => {
                // §10: On success: last_success = current result,
                //      current_attempt = current result, status = Current.
                self.last_success = Some(result.clone());
                self.current_attempt = Some(result);
                self.status = EvaluationStatus::Current;
                // A replacement can be much shorter than the previous output. Keeping
                // its old offset would render the new result below the viewport.
                self.output_scroll = 0;
                self.error_scroll = 0;
            }
            EvaluationKind::Empty => {
                self.current_attempt = Some(result);
                self.status = EvaluationStatus::Empty;
                self.output_scroll = 0;
                self.error_scroll = 0;
            }
            EvaluationKind::Failure | EvaluationKind::SyntaxError | EvaluationKind::SpawnError => {
                // §10: On failure: last_success = unchanged,
                //      current_attempt = failed result, status = Stale.
                self.current_attempt = Some(result);
                self.status = EvaluationStatus::Stale;
                // Stdout remains the prior successful result, but stderr was replaced.
                self.error_scroll = 0;
            }
        }
        true
    }

    /// Mark evaluation as started.
    pub fn mark_running(&mut self) {
        self.status = EvaluationStatus::Running;
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

    pub fn scroll_output_half_page_down(&mut self, page_height: usize, max: usize) {
        self.output_scroll = self.output_scroll.saturating_add(page_height / 2).min(max);
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

    pub fn scroll_error_half_page_down(&mut self, page_height: usize, max: usize) {
        self.error_scroll = self.error_scroll.saturating_add(page_height / 2).min(max);
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

    /// Scroll whichever pane is active according to §14.
    pub fn scroll_half_page_down(&mut self, page_height: usize, max: usize) {
        if self.scroll_target_is_error() {
            self.scroll_error_half_page_down(page_height, max);
        } else {
            self.scroll_output_half_page_down(page_height, max);
        }
    }

    /// Scroll whichever pane is active according to §14.
    pub fn scroll_half_page_up(&mut self, page_height: usize) {
        if self.scroll_target_is_error() {
            self.scroll_error_half_page_up(page_height);
        } else {
            self.scroll_output_half_page_up(page_height);
        }
    }

    /// Move whichever pane is active to its top.
    pub fn scroll_top(&mut self) {
        if self.scroll_target_is_error() {
            self.scroll_error_top();
        } else {
            self.scroll_output_top();
        }
    }

    /// Move whichever pane is active to its supplied maximum offset.
    pub fn scroll_bottom(&mut self, max: usize) {
        if self.scroll_target_is_error() {
            self.scroll_error_bottom(max);
        } else {
            self.scroll_output_bottom(max);
        }
    }
}

impl Default for OutputState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(
        generation: u64,
        kind: EvaluationKind,
        stdout: &str,
        stderr: &str,
    ) -> EvaluationResult {
        EvaluationResult {
            generation,
            command: "command".into(),
            stdout: stdout.into(),
            stderr: stderr.into(),
            exit_code: Some(if kind == EvaluationKind::Success {
                0
            } else {
                1
            }),
            kind,
        }
    }

    #[test]
    fn success_replaces_last_success() {
        let mut state = OutputState::new();
        state.apply_result(result(1, EvaluationKind::Success, "old", ""), 1);
        state.apply_result(result(2, EvaluationKind::Success, "new", "warning"), 2);

        assert_eq!(state.last_success.as_ref().unwrap().stdout, "new");
        assert_eq!(state.current_attempt.as_ref().unwrap().stderr, "warning");
        assert_eq!(state.status, EvaluationStatus::Current);
    }

    #[test]
    fn failure_preserves_last_success() {
        let mut state = OutputState::new();
        state.apply_result(result(1, EvaluationKind::Success, "kept", ""), 1);
        state.apply_result(result(2, EvaluationKind::Failure, "partial", "error"), 2);

        assert_eq!(state.last_success.as_ref().unwrap().stdout, "kept");
        assert_eq!(state.current_attempt.as_ref().unwrap().stderr, "error");
        assert_eq!(state.status, EvaluationStatus::Stale);
    }

    #[test]
    fn stale_generations_are_ignored() {
        let mut state = OutputState::new();
        state.apply_result(result(2, EvaluationKind::Success, "current", ""), 2);

        assert!(!state.apply_result(result(1, EvaluationKind::Success, "stale", ""), 2));
        assert_eq!(state.last_success.as_ref().unwrap().stdout, "current");
    }

    #[test]
    fn replacement_output_resets_scroll_to_top() {
        let mut state = OutputState::new();
        state.output_scroll = 100;
        state.error_scroll = 20;

        state.apply_result(result(2, EvaluationKind::Success, "short", ""), 2);

        assert_eq!(state.output_scroll, 0);
        assert_eq!(state.error_scroll, 0);
    }

    #[test]
    fn replacement_error_resets_only_error_scroll() {
        let mut state = OutputState::new();
        state.output_scroll = 10;
        state.error_scroll = 20;

        state.apply_result(result(2, EvaluationKind::Failure, "", "new error"), 2);

        assert_eq!(state.output_scroll, 10);
        assert_eq!(state.error_scroll, 0);
    }

    #[test]
    fn empty_result_sets_empty_without_replacing_last_success() {
        let mut state = OutputState::new();
        state.apply_result(result(1, EvaluationKind::Success, "kept", ""), 1);
        state.apply_result(result(2, EvaluationKind::Empty, "", ""), 2);

        assert_eq!(state.last_success.as_ref().unwrap().stdout, "kept");
        assert_eq!(state.status, EvaluationStatus::Empty);
    }

    #[test]
    fn output_scroll_actions_move_output() {
        let mut state = OutputState::new();

        state.scroll_half_page_down(20, 40); // Ctrl-D
        assert_eq!(state.output_scroll, 10);
        state.scroll_half_page_up(8); // Ctrl-U
        assert_eq!(state.output_scroll, 6);
        state.scroll_bottom(40); // G
        assert_eq!(state.output_scroll, 40);
        state.scroll_top(); // gg
        assert_eq!(state.output_scroll, 0);
    }

    #[test]
    fn visible_nonempty_stderr_targets_error_and_offsets_stay_independent() {
        let mut state = OutputState::new();
        state.output_scroll = 9;
        state.error_pane_visible = true;
        state.current_attempt = Some(result(1, EvaluationKind::Failure, "", "error"));

        state.scroll_half_page_down(20, 30);
        assert_eq!(state.output_scroll, 9);
        assert_eq!(state.error_scroll, 10);
        state.scroll_half_page_up(8);
        assert_eq!(state.error_scroll, 6);
        state.scroll_bottom(30);
        assert_eq!(state.error_scroll, 30);
        state.scroll_top();
        assert_eq!(state.error_scroll, 0);
        assert_eq!(state.output_scroll, 9);
    }

    #[test]
    fn hidden_or_empty_stderr_targets_output() {
        let mut state = OutputState::new();
        state.current_attempt = Some(result(1, EvaluationKind::Failure, "", "error"));
        state.scroll_half_page_down(10, 20);
        assert_eq!(state.output_scroll, 5);
        assert_eq!(state.error_scroll, 0);

        state.error_pane_visible = true;
        state.current_attempt = Some(result(2, EvaluationKind::Failure, "", ""));
        state.scroll_half_page_down(10, 20);
        assert_eq!(state.output_scroll, 10);
        assert_eq!(state.error_scroll, 0);
    }

    #[test]
    fn downward_scrolling_is_bounded_by_content() {
        let mut state = OutputState::new();
        state.scroll_half_page_down(20, 0);
        assert_eq!(state.output_scroll, 0);

        state.scroll_half_page_down(20, 3);
        assert_eq!(state.output_scroll, 3);
    }

    #[test]
    fn upward_scrolling_saturates_at_top() {
        let mut state = OutputState::new();
        state.output_scroll = 2;
        state.scroll_half_page_up(20);
        assert_eq!(state.output_scroll, 0);
    }
}
