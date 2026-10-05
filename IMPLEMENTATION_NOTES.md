# Silk — Implementation Notes & Checklist

## Overview

Silk is a Rust TUI for editing shell commands with live preview. Stack: `ratatui` + `crossterm` + `tui-textarea`. No fzf. No daemons.

---

## Phase 0 — Project Scaffolding ✅

- [x] `cargo init` — project root is `/workspace` (not a subdirectory)
- [x] Add dependencies to `Cargo.toml`:
  - `ratatui = "0.30"`
  - `crossterm = "0.29"`
  - `tui-textarea = "0.7"`
  - `serde = { version = "1", features = ["derive"] }`
  - `toml = "1"`
  - `anyhow = "1"`
- [x] Create `src/` module layout per §30 — all 10 files stubbed
- [x] Stub each module with `pub mod` declarations in `main.rs`
- [x] `cargo check` — compiles cleanly (only unused-import warnings from stubs)

---

## Phase 1 — Core Data Types & State Model (§4–6) ✅

### State types (`output.rs`)

- [x] `EvaluationKind` enum (§5):
  - `Empty`
  - `Success`
  - `Failure`
  - `SyntaxError`
  - `SpawnError`
- [x] `EvaluationResult` struct (§5):
  - `generation: u64`
  - `command: String`
  - `stdout: String`
  - `stderr: String`
  - `exit_code: Option<i32>`
  - `kind: EvaluationKind`
- [x] `EvaluationStatus` enum (§6):
  - `Empty`
  - `Running`
  - `Current`
  - `Stale`
- [x] `OutputState` struct (§4) — encapsulates output fields + scroll state:
  - `last_success: Option<EvaluationResult>`
  - `current_attempt: Option<EvaluationResult>`
  - `status: EvaluationStatus`
  - `output_scroll: usize`
  - `error_scroll: usize`
  - `error_pane_visible: bool`
- [x] `AppState` struct (in `app.rs`) — owns `EditorState` + `OutputState`
- [x] `EditorState` struct (in `editor.rs`):
  - text buffer backed by `tui_textarea::TextArea<'static>`
  - cursor position (via TextArea)
  - Vim mode: `VimMode` enum (`Normal` / `Insert`)
  - undo/redo history (built into TextArea)
- [x] `EditorEffect` enum (for action return values):
  - `None`
  - `Execute`
  - `KeepCommand`
  - `CopyOutput`
  - `CopyCommand`
  - `ToggleErrorPane`
  - `Cancel`

### State invariants (§10)

- [x] On success: `last_success = result`, `current_attempt = result`, `status = Current`
- [x] On failure/syntax-error: `last_success` unchanged, `current_attempt = result`, `status = Stale`
- [x] Failed command must **never** overwrite `last_success`

---

## Phase 2 — Configuration (§28) ✅

### `config.rs`

- [x] `Config` struct:
  - `input_mode: InputMode` (default `vim`)
  - `shell: String` (default `zsh`)
  - `debounce_ms: u64` (default `100`)
  - `clipboard_command: String` (default platform-appropriate)
  - `keybindings: HashMap<String, KeyChord>` (global actions)
  - `vim_keybindings: VimKeymap`
  - `environment: HashMap<String, String>`
- [x] `InputMode` enum: `Vim`, `Regular`
- [x] `VimKeymap` struct (§17):
  - `normal: HashMap<KeyChord, VimAction>`
  - `insert: HashMap<KeyChord, VimAction>`
- [x] Load from `~/.config/silk/config.toml` (or XDG path)
- [x] Merge user config over built-in defaults (partial override, not full replacement §18)
- [x] Silk must run without any config file present
- [x] Config parsing isolated from runtime — no config types leaking into UI/evaluator

### Default keybindings (§20–21)

- [x] Build default `VimKeymap.normal` with all actions from §20
- [x] Build default `VimKeymap.insert` with at least `Esc → Normal`
- [x] Build default global `keybindings` map from §21

### Phase 2 notes

- `Keymap` lives in `keymap.rs`; `Config` owns a resolved `Keymap` built by merging
  user overrides on top of defaults.
- `KeyChord` deserializes from config strings via a manual `Deserialize` impl that
  calls `FromStr`. Supported formats: single char (`a`, `;`, `$`), named keys
  (`enter`, `escape`/`esc`, `backspace`, `tab`, `delete`), modified keys
  (`ctrl-<a-z>`, `alt-<a-z>`).
- `KeyOrSequence` handles the `"gg"` double-tap sequence. `Keymap.gg_action` stores
  what global action `gg` triggers (default `ScrollTop`). `resolve_global()` checks
  for `gg` via a `prev_key` parameter.
- `VimMode` moved from `editor.rs` to `keymap.rs` to avoid circular deps between
  keymap ↔ editor.
- Config file path: `$XDG_CONFIG_HOME/silk/config.toml` or `~/.config/silk/config.toml`.
- Default clipboard command is platform-detected at runtime (pbcopy / wl-copy / xclip / clip.exe).

---

## Phase 3 — Key Parsing & Keymap (§17–19, §21)

### `keymap.rs`

- [ ] `KeyChord` struct (§19):
  - `code: KeyCode`
  - `modifiers: KeyModifiers`
- [ ] `VimAction` enum (§16):
  - `CursorLeft`, `CursorRight`
  - `BeginningOfLine`, `EndOfLine`
  - `WordForward`, `WordBackward`, `WordEnd`
  - `EnterInsertMode`, `AppendInsertMode`, `InsertAtBeginning`, `AppendAtEnd`
  - `DeleteChar`
  - `Undo`, `Redo`
  - `KeepCommand`
- [ ] `GlobalAction` enum (§21):
  - `ExecuteCommand` (Enter)
  - `Cancel` (Ctrl-C)
  - `ToggleErrorPane` (Ctrl-E)
  - `CopyOutput` (Ctrl-Y)
  - `CopyCommand` (Alt-Y)
  - `ScrollHalfPageDown` (Ctrl-D)
  - `ScrollHalfPageUp` (Ctrl-U)
  - `ScrollTop` (gg)
  - `ScrollBottom` (G)
  - `KeepCommand` (for regular mode)
- [ ] Key string parser: `"ctrl-r"`, `"alt-y"`, `";"`, `"$"`, `"enter"`, `"escape"`, `"a"`, `"A"` etc. (§19)
- [ ] `VimKeymap` lookup: given a key chord + current Vim mode → `Option<VimAction>`
- [ ] Global keymap lookup: given a key chord → `Option<GlobalAction>`
- [ ] Snake_case action names in config map directly to `VimAction` variants (§16)

### Extensibility requirement (§17)

- [ ] Adding a new `VimAction` requires **only**:
  1. Add variant to `VimAction` enum
  2. Add arm in `apply_vim_action()`
  3. Optionally add default keybinding to defaults
  4. Optionally document
- [ ] Config parser does **not** need a new field per action
- [ ] No fixed `VimBindings` struct with per-action fields

---

## Phase 4 — Editor (§16, §20)

### `editor.rs`

- [ ] `EditorState`:
  - text buffer (backed by `tui-textarea` or custom rope)
  - `vim_mode: VimMode` (Normal / Insert)
  - cursor
  - undo stack
- [ ] `apply_vim_action(action: VimAction, editor: &mut EditorState) -> EditorEffect`
  - Map each `VimAction` variant to concrete buffer/cursor edits
  - Return `EditorEffect` for actions that affect app-level state (e.g., `KeepCommand`)
- [ ] Insert mode behaviors (§20):
  - `Esc` → Normal mode
  - `Enter` → `EditorEffect::Execute`
  - `Ctrl-C` → cancellation warning (delegated to app)
  - `Ctrl-E` → toggle error pane (delegated to app)
- [ ] Normal mode motions (§20):
  - `h`, `l`, `w`, `b`, `e`, `0`, `$`
- [ ] Normal mode transitions:
  - `i`, `a`, `I`, `A`
- [ ] Normal mode editing:
  - `x` (delete char), `u` (undo), `Ctrl-R` (redo)
- [ ] Normal mode exit:
  - `q` → `EditorEffect::KeepCommand`
- [ ] `j` and `k` remain editor keys — **not** pane scrollers (§14)

### Keymap ↔ Editor separation

- [ ] `key → VimAction` resolution lives in `keymap.rs`
- [ ] `VimAction → editor behavior` lives in `editor.rs`
- [ ] These two concerns are not interleaved

---

## Phase 5 — Evaluator (§7–9)

### `evaluator.rs`

- [ ] `Evaluator` struct:
  - `shell: String`
  - `env_vars: HashMap<String, String>`
  - `generation: AtomicU64` (monotonically increasing)
  - handle to current running child process (for cancellation)
- [ ] `evaluate(command: &str, generation: u64, tx: Sender<EvaluationResult>)`
  - Must **not block** the UI thread — spawn async task or thread
  - Steps:
    1. Syntax check: `zsh -n -c "$command"` (configurable shell) — if syntax error, emit `EvaluationKind::SyntaxError` immediately
    2. Execute: `zsh -c 'setopt pipefail; eval "$1"' silk "$command"`
    3. Capture stdout, stderr, exit code **separately** (§7)
    4. Determine `EvaluationKind`
    5. Send `EvaluationResult` via channel
- [ ] Only results matching latest `generation` update app state (§9)
- [ ] Superseded evaluations: if a new generation starts before the previous completes, terminate the old child process if practical
- [ ] Debounce: 100ms default (§9) — implemented in `app.rs` event loop, not in evaluator

### Environment merging (§8)

- [ ] Inherit parent process environment
- [ ] Merge configured `[environment]` vars on top
- [ ] Nothing application-specific hardcoded

### Evaluator tests (§31)

- [ ] Success captures stdout
- [ ] Failure captures stderr
- [ ] Syntax error is not executed (no child spawned)
- [ ] Successful command with non-empty stderr retains stderr

---

## Phase 6 — Output State & Scrolling (§10, §14)

### `output.rs`

- [ ] `OutputState`:
  - `last_success: Option<EvaluationResult>`
  - `current_attempt: Option<EvaluationResult>`
  - `status: EvaluationStatus`
  - `output_scroll: usize`
  - `error_scroll: usize`
  - `error_pane_visible: bool`
- [ ] `apply_result(state: &mut OutputState, result: EvaluationResult, current_gen: u64)`:
  - Ignore if `result.generation < current_gen` (§9)
  - Update per §10 semantics
- [ ] Scroll methods:
  - `scroll_output_half_page_down()`
  - `scroll_output_half_page_up()`
  - `scroll_output_top()`
  - `scroll_output_bottom()`
  - `scroll_error_half_page_down()`
  - `scroll_error_half_page_up()`
  - `scroll_error_top()`
  - `scroll_error_bottom()`
- [ ] Scrolling target logic (§14):
  - If error pane visible **and** stderr is non-empty → scroll error pane
  - Otherwise → scroll output pane
- [ ] Output and error scroll offsets are **independent**

### State tests (§31)

- [ ] Success replaces `last_success`
- [ ] Failure preserves `last_success`
- [ ] Stale generations are ignored

### Scrolling tests (§31)

- [ ] Ctrl-D, Ctrl-U, gg, G each move scroll correctly
- [ ] Output and error offsets remain independent
- [ ] `cargo test output` — verify state + scrolling tests pass  ← **relay via agentq**

---

## Phase 7 — Clipboard (§26–27)

### `clipboard.rs`

- [ ] `Clipboard` struct:
  - `command: String` (e.g., `"pbcopy"`, `"wl-copy"`, `"xclip"`)
- [ ] `copy(text: &str)`:
  - Pipe `text` to external clipboard command via stdin
  - Handle errors gracefully (show message if clipboard fails)
- [ ] `copy_output(state: &OutputState)`:
  - Copy `last_success.stdout` if present
- [ ] `copy_command(editor: &EditorState)`:
  - Copy current editor text
- [ ] Clipboard logic must **not** be coupled to renderer (§27)
- [ ] After copy, show short-lived "copied" message (§26)

---

## Phase 8 — UI Rendering (§11–13)

### `ui.rs`

- [ ] Renderer takes `&AppState` and returns nothing else — pure function of state (§3)
- [ ] Renderer must **not**:
  - Execute commands
  - Mutate editor
  - Access clipboard
  - Own evaluation logic
- [ ] Default layout (§11):
  - Top: output area (scrollable)
  - Middle: editor bar with mode indicator + command text
  - Bottom: key hints / status messages
- [ ] Error pane layout (§11):
  - 70% stdout (left), 30% stderr (right)
  - Only when `error_pane_visible == true`
- [ ] Border styling (§13):
  - Green border → `Current`
  - Yellow border → `Running`
  - Gray border → `Empty` or `Stale`
  - **Never red** for failures
- [ ] Hidden stderr indicator (§12):
  - When stderr non-empty and pane hidden: small `! stderr` indicator
  - Not visually dominant
- [ ] Empty stdout renders `(no output)` in subdued gray (§10)
- [ ] Independent scroll offsets for output and error panes
- [ ] Status bar showing: mode (INSERT/NORMAL), evaluation status text
- [ ] Key hints bar at bottom

---

## Phase 9 — App Controller (§22–25)

### `app.rs`

- [ ] `App` struct owns:
  - `AppState`
  - `Config`
  - `Evaluator`
  - `Clipboard`
  - Terminal handle
- [ ] Main event loop:
  1. Read input events (crossterm)
  2. Debounce timer for evaluation (100ms)
  3. Route keys through keymap → `VimAction` or `GlobalAction`
  4. Apply actions to editor/output state
  5. Trigger evaluation on edit
  6. Receive evaluation results from channel (non-blocking)
  7. Render UI
- [ ] **Enter** handling (§22):
  - Restore terminal
  - Print command to stdout
  - `exit(10)`
- [ ] **q** (Normal mode) / keep-command binding (§23):
  - Print edited command to stdout
  - `exit(0)`
- [ ] **Ctrl-C** cancellation (§24):
  - First press: show "Press Ctrl-C again to cancel"
  - Second consecutive press: restore terminal, `exit(130)`
  - Any other input clears pending cancellation state
- [ ] **Esc** never exits Silk (§25):
  - Vim Insert → Normal
  - Normal mode: clear pending state
  - Regular mode: no-op
- [ ] Generation tracking: each edit increments generation counter, passes to evaluator
- [ ] Stale result rejection: compare `result.generation` against current before applying

### Protocol tests (§31)

- [ ] `q` → exit 0
- [ ] `Enter` → exit 10
- [ ] double `Ctrl-C` → exit 130
- [ ] `cargo test protocol` — verify protocol tests pass  ← **relay via agentq**

---

## Phase 10 — Shell Protocol (§29)

### `protocol.rs`

- [ ] Parse `--query "$BUFFER"` argument
- [ ] On exit, write **only** the command to stdout (for exit codes 0 and 10)
- [ ] UI/debug output must **never** pollute stdout — use stderr for all diagnostics
- [ ] Exit codes:
  - `0` — keep edited command
  - `10` — execute edited command
  - `130` — cancel, restore original buffer
- [ ] Terminal state always restored on exit (use RAII guard or `Drop`)

> **Testing note:** All `cargo test` / `cargo build` / `cargo check` commands must be
> relayed via `agentq push` — they cannot run in the sandbox. Write test code directly,
> then push the run command to the user.

---

## Phase 11 — Vim Action Tests (§31)

- [ ] Write unit tests for Vim actions
- [ ] `cargo test` — verify all Vim action tests pass  ← **relay via agentq**

- [ ] Esc → Normal mode
- [ ] i → Insert mode
- [ ] a / A / I behaviors correct
- [ ] h / l cursor movement
- [ ] w / b / e word movement
- [ ] Configurable binding test:
  - Set `end_of_line = ";"`
  - Verify `;` invokes `EndOfLine`
  - Verify `$` no longer invokes `EndOfLine`
- [ ] Extensibility test:
  - Add a test `VimAction` variant
  - Verify: config name → keymap → action → editor behavior
  - Confirm no changes needed to config struct

---

## Phase 12 — Cancellation Tests (§31)

- [ ] First Ctrl-C shows warning message
- [ ] Second Ctrl-C cancels (exit 130)
- [ ] Typing any other key after first Ctrl-C clears pending cancellation
- [ ] Cancellation restores terminal state
- [ ] `cargo test` — verify cancellation tests pass  ← **relay via agentq**

---

## Phase 13 — Integration & Polish

- [ ] Startup feels immediate (no perceptible delay)
- [ ] Current shell buffer opens in editor (via `--query`)
- [ ] Vim mode enabled by default
- [ ] Regular input mode configurable via `input_mode = "regular"`
- [ ] All default keybindings work without config file
- [ ] User config merges over defaults (partial overrides)
- [ ] `Ctrl-E` toggles error pane, stays open until explicitly closed
- [ ] Output copying works (Ctrl-Y)
- [ ] Command copying works (Alt-Y)
- [ ] Scroll bindings work (Ctrl-D, Ctrl-U, gg, G)
- [ ] `j` / `k` are editor-only, not pane scrollers
- [ ] Terminal state restored on all exit paths (panic hook too)
- [ ] No fzf in dependency tree
- [ ] No daemon or background process
- [ ] `cargo build --release` — verify release build  ← **relay via agentq**
- [ ] Manual smoke test: `silk --query "echo hello"`  ← **relay via agentq**

---

## Implementation Order Recommendation

1. **Phase 0** — scaffolding
2. **Phase 1** — data types (no behavior yet)
3. **Phase 2** — config loading (can test in isolation)
4. **Phase 3** — key parsing + keymap (can test with unit tests)
5. **Phase 4** — editor + Vim actions (can test headlessly)
6. **Phase 5** — evaluator (can test with shell commands)
7. **Phase 6** — output state + scrolling (pure logic, testable)
8. **Phase 7** — clipboard (simple, testable)
9. **Phase 8** — UI rendering (last, depends on everything else)
10. **Phase 9** — app controller (wires everything together)
11. **Phase 10** — shell protocol (exit codes, stdout contract)
12. **Phases 11–13** — tests + polish

---

## Notes

- `tui-textarea` provides built-in Vim emulation; verify it supports the required Vim action model from §16–20. If it couples keybindings to behavior too tightly, a custom editor may be needed to satisfy the separation requirements of §17.
- The `generation` counter must be atomic (`AtomicU64`) since evaluation runs on a separate thread.
- Use `mpsc::channel` or `tokio::sync::mpsc` for evaluator → app communication. Keep async runtime choice minimal — `tokio` may be overkill if only one background thread is needed.
- For debounce, use `Instant::elapsed()` checks in the event loop rather than async timers to keep dependencies minimal.
- Terminal restore: use a guard struct with `Drop` that calls `crossterm::terminal::disable_raw_mode()` and `LeaveAlternateScreen`. Also register a panic hook.
- Project root is `/workspace` (not a subdirectory).
- **Agent workflow:** All `cargo` commands must be relayed via `agentq push $PI_SESSION_ID '<cmd>'`. File edits are done directly in the sandbox.

### Phase 1 notes

- `OutputState` groups all output-related fields (last_success, current_attempt, status, scroll
  offsets, error_pane_visible) into a single struct rather than flattening them into `AppState`
  directly. This keeps the state model cleaner and gives output logic a clear home.
- `EditorState` uses `TextArea<'static>` (owned content) — no lifetime propagation needed in
  parent structs.
- `EditorEffect` uses flat variants (`Execute`, `KeepCommand`, `Cancel`, etc.) rather than
  `Exit(i32)`. The exit codes (0, 10, 130) will be handled by the app controller when it
  processes these effects.

