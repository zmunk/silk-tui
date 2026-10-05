# silk-widget.zsh — ZLE widget that opens the current buffer in Silk.
#
# Install:
#   ./install.sh
# or source this file and run `zle -N silk-widget` manually.
#
# Exit-code contract (see SPEC §29):
#   exit  0  → keep the edited command in the buffer (no execution)
#   exit 10  → accept and execute the edited command
#   exit 130 → cancel: restore the original buffer and cursor

silk-widget() {
  local _silk__original_buffer="${BUFFER}"
  local _silk__original_cursor="${CURSOR}"
  local _silk__exit_code=0

  # Run Silk with the current buffer as the pre-filled query.
  # Output is captured into the widget's own local scope so the shell
  # buffer and cursor are not disturbed until we assign them below.
  local _silk__result
  # ZLE may give external commands non-terminal stdin; keyboard input must
  # remain on the controlling TTY while stdout is captured for the protocol.
  _silk__result="$(silk --query "${_silk__original_buffer}" </dev/tty)" || _silk__exit_code=$?

  case ${_silk__exit_code} in
    0|10) ;; # Only successful keep/execute results may replace the buffer.
    130)
      BUFFER="${_silk__original_buffer}"
      CURSOR="${_silk__original_cursor}"
      zle redisplay
      return 0
      ;;
    *)
      BUFFER="${_silk__original_buffer}"
      CURSOR="${_silk__original_cursor}"
      print -u2 -r -- "silk: unexpected exit code ${_silk__exit_code}"
      zle redisplay
      return 1
      ;;
  esac

  # Update the buffer with whatever Silk wrote to stdout.
  BUFFER="${_silk__result}"

  # Clamp the cursor so it never exceeds the new buffer length.
  if (( ${#BUFFER} < _silk__original_cursor )); then
    CURSOR="${#BUFFER}"
  else
    CURSOR="${_silk__original_cursor}"
  fi
  zle redisplay

  # Keep-only mode: buffer updated, user stays at the prompt.
  if (( _silk__exit_code == 0 )); then
    return 0
  fi

  # Execute mode: accept the line as if the user pressed Enter.
  if (( _silk__exit_code == 10 )); then
    zle accept-line
  fi
}

if [[ -o interactive ]]; then
  zle -N silk-widget
fi
