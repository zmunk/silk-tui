#!/usr/bin/env bash
# Install the Silk zsh widget into the user's environment.
#
# Behavior:
#   1. Copies contrib/silk-widget.zsh into $SILK_HOME (default ~/.local/share/silk).
#   2. Appends a `source …` line to ~/.zshrc if one is not already present.
#   3. Prints instructions for the current shell session.
#
# Override the install directory with $SILK_HOME.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
WIDGET_SRC="${SCRIPT_DIR}/contrib/silk-widget.zsh"
ZSHRC="${ZDOTDIR:-$HOME}/.zshrc"

if [[ ! -f "${WIDGET_SRC}" ]]; then
  echo "error: ${WIDGET_SRC} not found" >&2
  exit 1
fi

SILK_HOME="${SILK_HOME:-$HOME/.local/share/silk}"

# --- copy widget into $SILK_HOME ---
mkdir -p "${SILK_HOME}"
cp -f "${WIDGET_SRC}" "${SILK_HOME}/silk-widget.zsh"

# --- ensure ~/.zshrc contains a source line ---
SOURCE_LINE="source \"${SILK_HOME}/silk-widget.zsh\""
BINDLINE="bindkey '^X^S' silk-widget"

if grep -qF "${SOURCE_LINE}" "${ZSHRC}" 2>/dev/null; then
  echo "silk: ${SOURCE_LINE} already present in ${ZSHRC}"
else
  echo "" >> "${ZSHRC}"
  echo "# Silk zsh widget" >> "${ZSHRC}"
  echo "${SOURCE_LINE}" >> "${ZSHRC}"
  echo "silk: appended ${SOURCE_LINE} to ${ZSHRC}"
fi

if grep -qF "${BINDLINE}" "${ZSHRC}" 2>/dev/null; then
  echo "silk: ${BINDLINE} already present in ${ZSHRC}"
else
  echo "${BINDLINE}" >> "${ZSHRC}"
  echo "silk: appended ${BINDLINE} to ${ZSHRC}"
fi

# --- report ---
echo ""
echo "silk widget installed to ${SILK_HOME}/silk-widget.zsh"
echo ""
echo "To use it in this shell session, run:"
echo "  source ${SOURCE_LINE}"
echo ""
echo "Next time your shell starts it will be loaded automatically."
