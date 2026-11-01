# shellcheck shell=bash
# Shared by the Mortar AI scripts (sourced). The pad types keys in the
# terminal: A Enter, B Backspace, X Tab, Y Space, L2 Esc, D-pad arrows.

# Signed in to Claude Code? Credentials live in ~/.claude/.credentials.json
# (mode 0600, on the root file system); an API key or a setup-token in the
# environment also counts. Checking the file avoids starting claude (~8 s on
# the Brick) just to ask.
signed_in() {
  [ -n "${ANTHROPIC_API_KEY:-}${CLAUDE_CODE_OAUTH_TOKEN:-}" ] && return 0
  [ -s "${CLAUDE_CONFIG_DIR:-$HOME/.claude}/.credentials.json" ]
}

# Wait for one button; prints a, b, x, y, esc, up, down, left, right or the
# typed character.
read_button() {
  local k rest=""
  IFS= read -rsn1 k || { echo eof; return; }
  case "$k" in
    "") echo a ;;
    $'\t') echo x ;;
    " ") echo y ;;
    $'\x7f' | $'\b') echo b ;;
    $'\e')
      IFS= read -rsn2 -t 0.05 rest || true
      case "$rest" in
        "[A") echo up ;;
        "[B") echo down ;;
        "[C") echo right ;;
        "[D") echo left ;;
        "") echo esc ;;
        *) read -rs -t 0.05 _ || true; echo other ;;
      esac
      ;;
    *) printf '%s\n' "$k" ;;
  esac
}

bold() { printf '\e[1m%s\e[0m' "$*"; }
dim() { printf '\e[2m%s\e[0m' "$*"; }
# A button cap like the shell's hint bar: black on lime.
cap() { printf '\e[30;48;2;206;240;88m %s \e[0m' "$1"; }

# Clear the screen and scrollback (no ncurses `clear` needed).
cls() { printf '\033[H\033[2J\033[3J'; }

heading() {
  cls
  printf '\n  \e[1m%s\e[0m\n\n' "$1"
}
