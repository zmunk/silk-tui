# Silk — Implementation Specification

## 1. Purpose

Silk is a lightweight terminal UI for editing shell commands while continuously previewing their output.

Silk:

* edits a command;
* evaluates it live;
* preserves the last successful stdout when the current command fails;
* shows stderr separately;
* returns the final command to the calling shell;
* optionally tells the shell to execute it.

Silk does not replace the shell. Final command execution remains the shell's responsibility.

---

## 2. Stack

Implement Silk in Rust using:

* `ratatui` — TUI rendering;
* `crossterm` — terminal input/raw mode;
* `tui-textarea` or an equivalent lightweight editor component;
* standard Rust process APIs for command execution.

Do not use fzf.

Avoid unnecessary frameworks, daemons, plugin systems, or background services.

---

## 3. Architecture

Prefer small, loosely coupled, replaceable components.

Suggested boundaries:

```text
App
├── Editor
├── Evaluator
├── OutputState
├── Renderer
├── Clipboard
├── Config
└── ShellProtocol
```

Components should communicate through small explicit interfaces rather than depending directly on each other's implementations.

The renderer must only render state.

It must not:

* execute commands;
* mutate editor contents;
* access the clipboard;
* own evaluation logic.

The editor keymap layer must also be separate from editor action execution so keybindings can be remapped without changing editing behavior.

---

## 4. Core State Model

Silk must separately track:

```rust
struct AppState {
    editor: EditorState,
    last_success: Option<EvaluationResult>,
    current_attempt: Option<EvaluationResult>,
    evaluation_status: EvaluationStatus,
    output_scroll: usize,
    error_scroll: usize,
    error_pane_visible: bool,
}
```

The distinction between:

```text
last_success
current_attempt
```

is mandatory.

A failed command must never overwrite the last successful stdout.

---

## 5. Evaluation Result

```rust
struct EvaluationResult {
    generation: u64,
    command: String,
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    kind: EvaluationKind,
}
```

```rust
enum EvaluationKind {
    Empty,
    Success,
    Failure,
    SyntaxError,
    SpawnError,
}
```

---

## 6. Evaluation Status

```rust
enum EvaluationStatus {
    Empty,
    Running,
    Current,
    Stale,
}
```

Meaning:

```text
Empty    current command is empty
Running  current command is being evaluated
Current  displayed stdout belongs to current command
Stale    displayed stdout belongs to an earlier successful command
```

Failures and syntax errors result in `Stale`.

---

## 7. Command Evaluation

Commands are evaluated through configurable shell execution.

Default:

```text
shell = zsh
```

Before execution, perform syntax validation equivalent to:

```sh
zsh -n -c "$command"
```

If valid, execute with semantics equivalent to:

```sh
zsh -c '
    setopt pipefail
    eval "$1"
' silk "$command"
```

Capture separately:

```text
stdout
stderr
exit code
```

Do not merge stderr into stdout.

---

## 8. Evaluation Environment

Silk must support arbitrary environment variables applied to preview commands.

Nothing application-specific should be hardcoded.

Example:

```toml
[environment]
RG_WRAPPER_DISABLE = "1"
NO_COLOR = "1"
MY_CUSTOM_VARIABLE = "value"
```

Configured variables are merged over the environment inherited by Silk.

---

## 9. Live Evaluation

Editing the command triggers evaluation.

Default debounce:

```text
100 ms
```

The initial command should be evaluated immediately.

Evaluation must not block the UI.

Each edit receives a monotonically increasing generation ID.

Only results belonging to the latest generation may update current state.

Older results must be ignored.

If practical, terminate superseded evaluation processes.

---

## 10. Output Semantics

On success:

```text
last_success = current result
current_attempt = current result
status = Current
```

On failure or syntax error:

```text
last_success = unchanged
current_attempt = failed result
status = Stale
```

Silk can therefore display:

```text
last successful stdout
+
current stderr
```

simultaneously.

A successful command with empty stdout is still successful.

Render:

```text
(no output)
```

using a subdued gray style.

---

## 11. Layout

Default:

```text
┌───────────────────────────────────────────────────────────┐
│                                                           │
│                     OUTPUT                                │
│                                                           │
├───────────────────────────────────────────────────────────┤
│ INSERT │ command being edited                             │
├───────────────────────────────────────────────────────────┤
│ key hints / temporary messages                            │
└───────────────────────────────────────────────────────────┘
```

With the error pane enabled:

```text
┌──────────────────────────────────────┬────────────────────┐
│ OUTPUT                               │ ERRORS             │
│                                      │                    │
│                                      │                    │
├──────────────────────────────────────┴────────────────────┤
│ NORMAL │ command being edited                            │
├───────────────────────────────────────────────────────────┤
│ key hints                                                │
└───────────────────────────────────────────────────────────┘
```

Default split:

```text
stdout 70%
stderr 30%
```

No pane focus or pane navigation is required.

---

## 12. Error Pane

Global key:

```text
Ctrl-E
```

toggles the error pane.

The pane remains open until explicitly closed, even when stderr becomes empty.

If stderr is non-empty while the pane is hidden, display a small non-distracting indicator such as:

```text
stderr
```

or:

```text
! stderr
```

Do not make it visually dominant.

---

## 13. Status Styling

Use the main output border as the primary evaluation indicator.

Recommended:

```text
green   Current
yellow  Running
gray    Empty or Stale
```

Do not use red for failures.

Do not rely exclusively on color. A subtle textual status may also be shown.

---

## 14. Scrolling

Output and error panes maintain independent scroll offsets.

No pane selection is required.

When the error pane is visible and contains stderr, scrolling commands operate on the error pane.

Otherwise they operate on stdout.

Required bindings:

```text
Ctrl-D    half page down
Ctrl-U    half page up
gg        top
G         bottom
```

`j` and `k` do not scroll output.

They remain part of editor/Vim behavior.

---

## 15. Input Modes

Input mode is configurable.

Default:

```toml
input_mode = "vim"
```

Supported modes:

```text
vim
regular
```

`regular` provides normal non-modal text input.

`vim` provides Insert and Normal modes.

---

## 16. Vim Action Model

Vim behavior must be represented as semantic actions rather than hardcoded key checks.

Example:

```rust
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "snake_case")]
enum VimAction {
    CursorLeft,
    CursorRight,

    BeginningOfLine,
    EndOfLine,

    WordForward,
    WordBackward,
    WordEnd,

    EnterInsertMode,
    AppendInsertMode,
    InsertAtBeginning,
    AppendAtEnd,

    DeleteChar,

    Undo,
    Redo,

    KeepCommand,
}
```

Configuration names therefore derive directly from action names:

```text
CursorLeft         → cursor_left
BeginningOfLine    → beginning_of_line
EndOfLine          → end_of_line
WordForward        → word_forward
```

The config must use these snake_case action names directly.

---

## 17. Maintainable Vim Action Extension

Adding a new Vim action should require minimal localized changes.

The architecture should separate:

```text
key → VimAction
```

from:

```text
VimAction → editor behavior
```

Recommended structure:

```rust
struct VimKeymap {
    normal: HashMap<KeyChord, VimAction>,
    insert: HashMap<KeyChord, VimAction>,
}
```

Action execution:

```rust
fn apply_vim_action(
    action: VimAction,
    editor: &mut EditorState,
) -> EditorEffect
```

Adding a new action should normally require only:

1. adding a `VimAction` enum variant;
2. implementing it in `apply_vim_action`;
3. optionally adding a default keybinding;
4. optionally documenting it.

The config parser must not require a new custom field for every action.

Avoid configuration types such as:

```rust
struct VimBindings {
    beginning_of_line: Option<String>,
    end_of_line: Option<String>,
    word_forward: Option<String>,
    ...
}
```

because that makes every new action require config-structure changes.

Instead, use a generic action map.

---

## 18. Vim Keybinding Configuration

Recommended TOML:

```toml
[vim_keybindings.normal]
beginning_of_line = "0"
end_of_line = ";"
word_forward = "w"
word_backward = "b"
word_end = "e"
cursor_left = "h"
cursor_right = "l"
enter_insert_mode = "i"
append_insert_mode = "a"
insert_at_beginning = "I"
append_at_end = "A"
delete_char = "x"
undo = "u"
redo = "ctrl-r"
keep_command = "q"
```

Insert-mode overrides may use:

```toml
[vim_keybindings.insert]
```

only when needed.

Defaults are built into Silk.

User configuration overrides individual actions rather than replacing the entire keymap.

For example:

```toml
[vim_keybindings.normal]
end_of_line = ";"
```

must leave all other defaults intact.

---

## 19. Key Parsing

Key configuration must support at least:

```text
a
A
;
$
enter
escape
ctrl-r
ctrl-d
ctrl-u
ctrl-y
ctrl-e
alt-y
```

Represent parsed keys using a normalized internal type such as:

```rust
struct KeyChord {
    code: KeyCode,
    modifiers: KeyModifiers,
}
```

Parsing belongs in the config/keymap layer, not the editor.

---

## 20. Required Vim Behavior

### Insert mode

```text
Esc       Normal mode
Enter     execute command
Ctrl-C    cancellation warning
Ctrl-E    toggle error pane
```

### Normal mode

Default motions:

```text
h       cursor left
l       cursor right
w       next word
b       previous word
e       end of word
0       beginning of line
$       end of line
```

Default transitions:

```text
i       insert
a       append
I       insert at beginning
A       append at end
```

Default editing:

```text
x       delete character
u       undo
Ctrl-R  redo
```

Exit without execution:

```text
q
```

All action keys above must be remappable through the action-based keymap.

---

## 21. Global Keys

Some actions are application-wide rather than Vim editor actions.

Recommended defaults:

```text
Enter       execute command
Ctrl-C      cancellation warning / cancel
Ctrl-E      toggle error pane
Ctrl-Y      copy current output
Alt-Y       copy current command
Ctrl-D      half-page scroll down
Ctrl-U      half-page scroll up
G           scroll bottom
gg          scroll top
```

These should also be configurable, but separately from `vim_keybindings`.

Example:

```toml
[keybindings]
copy_output = "ctrl-y"
copy_command = "alt-y"
toggle_error_pane = "ctrl-e"
scroll_half_page_down = "ctrl-d"
scroll_half_page_up = "ctrl-u"
scroll_bottom = "G"
scroll_top = "gg"
```

Global application actions and Vim editor actions should remain conceptually separate.

---

## 22. Command Submission

`Enter` means:

```text
accept current command
request execution by shell
```

Silk:

1. restores terminal state;
2. prints the command to stdout;
3. exits with code `10`.

The calling shell executes the returned command normally.

---

## 23. Keep Command

In Vim Normal mode, default:

```text
q
```

means:

```text
return edited command without executing it
```

Silk:

```text
stdout = edited command
exit code = 0
```

For regular mode, provide an equivalent configurable global binding.

---

## 24. Cancellation

The first:

```text
Ctrl-C
```

must not immediately discard work.

Show:

```text
Press ctrl-c again to cancel and discard current command
```

The second consecutive `Ctrl-C` cancels Silk.

Cancellation:

```text
stdout = nothing
exit code = 130
```

The shell integration restores the original buffer and cursor.

Any meaningful input after the first Ctrl-C clears the pending cancellation state.

---

## 25. Escape

`Esc` must never close Silk.

In Vim mode:

```text
Insert → Normal
```

In Normal mode, it may clear pending Vim state.

In regular input mode, it should do nothing unless configured.

---

## 26. Copying

Provide separate actions for:

```text
copy current output
copy current command
```

Recommended defaults:

```text
Ctrl-Y    copy current output
Alt-Y     copy current command
```

"Current output" means:

```text
last successful stdout
```

After copying, show a short-lived:

```text
copied
```

message.

---

## 27. Clipboard

Clipboard functionality must be replaceable.

Prefer an external clipboard command.

Examples:

```text
copy
pbcopy
wl-copy
xclip
clip.exe
```

Allow explicit configuration:

```toml
clipboard_command = "copy"
```

Do not couple clipboard logic to the renderer.

---

## 28. Configuration

A minimal configuration:

```toml
input_mode = "vim"
shell = "zsh"
debounce_ms = 100
clipboard_command = "copy"

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
```

Defaults must allow Silk to run without a config file.

Configuration parsing should remain isolated from runtime behavior.

---

## 29. Shell Integration Contract

Silk accepts the current shell buffer:

```sh
silk --query "$BUFFER"
```

Exit meanings:

```text
0     keep edited command
10    execute edited command
130   cancel and restore original buffer
other unexpected Silk failure
```

stdout contains only the returned command for exit codes `0` and `10`.

Silk UI/debug output must never pollute stdout.

---

## 30. Required Modules

A minimal source layout:

```text
src/
├── main.rs
├── app.rs
├── config.rs
├── keymap.rs
├── editor.rs
├── evaluator.rs
├── output.rs
├── clipboard.rs
├── ui.rs
└── protocol.rs
```

Responsibilities:

```text
config.rs     load and merge configuration
keymap.rs     parse keys and map them to semantic actions
editor.rs     editor state and VimAction execution
evaluator.rs  syntax checking and command execution
output.rs     last-success/current-attempt state and scrolling
clipboard.rs  copy abstraction
ui.rs         rendering only
protocol.rs   shell exit/output contract
```

Do not split further unless complexity justifies it.

---

## 31. Minimum Tests

### evaluator

```text
success captures stdout
failure captures stderr
syntax error is not executed
successful stderr is retained
```

### state

```text
success replaces last_success
failure preserves last_success
stale generations are ignored
```

### scrolling

```text
Ctrl-D
Ctrl-U
gg
G
output/error offsets remain independent
```

### Vim actions

```text
Esc → Normal
i → Insert
a / A / I
h / l
w / b / e
```

### configurable Vim bindings

Given:

```toml
[vim_keybindings.normal]
end_of_line = ";"
```

verify:

```text
; invokes EndOfLine
$ no longer invokes EndOfLine unless separately mapped
```

### action extensibility

Add a test-only or simple additional `VimAction` and verify that:

```text
config name
→ keymap
→ action
→ editor behavior
```

works without changes to a fixed config struct.

### cancellation

```text
first Ctrl-C shows warning
second Ctrl-C cancels
other input clears pending cancellation
```

### shell protocol

```text
q → exit 0
Enter → exit 10
double Ctrl-C → exit 130
```

---

## 32. Definition of Done

Silk is ready when:

* startup feels immediate;
* the current shell buffer opens in the editor;
* Vim mode is enabled by default;
* regular input mode can be configured;
* Vim actions are represented semantically;
* Vim keymap config uses snake_case action names such as `beginning_of_line` and `end_of_line`;
* supported Vim actions can be remapped individually;
* adding new Vim actions requires minimal localized code changes;
* edits trigger debounced non-blocking evaluation;
* stdout and stderr are captured separately;
* failure preserves last successful stdout;
* Ctrl-E toggles the stderr split;
* hidden stderr is indicated unobtrusively;
* output/error scrolling supports Ctrl-D, Ctrl-U, gg, and G;
* `j` and `k` remain editor keys rather than pane-scrolling keys;
* current output can be copied;
* current command can be copied;
* Enter returns and executes the command through the shell;
* Normal-mode `q` returns the command without execution;
* double Ctrl-C cancels and restores the original shell buffer;
* Esc never exits Silk;
* terminal state is always restored on exit;
* no fzf dependency exists;
* no daemon or persistent background process exists.
