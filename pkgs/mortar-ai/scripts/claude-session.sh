# Runs inside the "claude" tmux session (see claude-attach.sh): first-run
# sign-in, then Claude Code, then a small menu when it exits. The tmux
# session (and with it Claude Code) keeps running when the terminal app is
# closed or switched away from.
#
# Env: MORTAR_AI_LIB (common.sh), MORTAR_AI_SOCKET (tmux -L name),
# MORTAR_AI_WORKDIR.
# shellcheck source=/dev/null
. "$MORTAR_AI_LIB"

sock=$MORTAR_AI_SOCKET
pane=${TMUX_PANE:-}

# Watch this pane for the sign-in link `claude auth login` prints, then show
# it as a QR code with a phone page to paste the code into (mortar-ai pair),
# and type the code it returns into the waiting login prompt.
watch_login_url() {
  local url="" code
  for _ in $(seq 600); do
    sleep 0.5
    url=$(tmux -L "$sock" capture-pane -p -J -t "$pane" -S -60 2>/dev/null |
      grep -o "visit: https://[^[:space:]]*" | tail -n 1 | cut -d' ' -f2) || true
    [ -n "$url" ] && break
  done
  [ -n "$url" ] || return 0
  # the pane's environment may predate a shell restart; ask tmux
  eval "$(tmux -L "$sock" show-environment -s WAYLAND_DISPLAY 2>/dev/null || true)"
  eval "$(tmux -L "$sock" show-environment -s XDG_RUNTIME_DIR 2>/dev/null || true)"
  if code=$(mortar-ai pair --url "$url" 2>/dev/null) && [ -n "$code" ]; then
    tmux -L "$sock" send-keys -t "$pane" -l "$code"
    tmux -L "$sock" send-keys -t "$pane" Enter
  fi
}

login_flow() {
  while ! signed_in; do
    heading "Sign in to Claude"
    cat <<'EOF'
  Claude Code needs your Claude account (Pro, Max, Team or Enterprise;
  Remote Control needs one of those plans). The sign-in is saved on this
  Brick in ~/.claude, on the Linux partition, readable only by you.

  What happens next:
    1. A QR code appears. Scan it with your phone (same Wi-Fi as the Brick).
    2. Sign in to Claude on the page that opens.
    3. Claude shows a code. Paste it into that same page and tap Send;
       the Brick types it in here.

  Phone not on this Wi-Fi? Press X on the QR screen for the plain sign-in
  link, then type the code here yourself (START opens the keyboard).
EOF
    printf '  From a computer instead: ssh root@mortar.local, then run\n'
    printf "  \`su - %s -c 'claude auth login'\`.\n\n" "$(id -un)"
    printf '  %s continue    %s skip for now\n' "$(cap A)" "$(cap B)"
    case "$(read_button)" in
      a) ;;
      b | esc | eof) return 1 ;;
      *) continue ;;
    esac
    heading "Sign in to Claude"
    watch_login_url &
    local watcher=$!
    # BROWSER: there is no browser here; a no-op keeps claude from trying
    # xdg-open, and it prints the link instead.
    BROWSER=true claude auth login --claudeai || true
    kill "$watcher" 2>/dev/null || true
    wait "$watcher" 2>/dev/null || true
    if signed_in; then
      printf '\n  Signed in. Starting Claude Code…\n'
      sleep 1
    else
      printf '\n  Not signed in. %s try again    %s skip\n' "$(cap A)" "$(cap B)"
      case "$(read_button)" in
        b | esc | eof) return 1 ;;
      esac
    fi
  done
}

mkdir -p "$MORTAR_AI_WORKDIR"
cd "$MORTAR_AI_WORKDIR" || exit 1

args=()
while true; do
  signed_in || login_flow || true
  cls
  claude "${args[@]}" || true
  args=()
  heading "Claude Code ended"
  printf '  %s start again\n  %s continue the last conversation\n  %s shell in %s\n\n' \
    "$(cap A)" "$(cap X)" "$(cap Y)" "$(dim "$MORTAR_AI_WORKDIR")"
  printf '  %s\n' "$(dim 'The session keeps running in the background; MENU switches apps.')"
  case "$(read_button)" in
    x) args=(--continue) ;;
    y)
      cls
      printf 'Shell in %s. Type exit to return to the menu.\n' "$MORTAR_AI_WORKDIR"
      bash -l || true
      ;;
    eof) exit 0 ;;
  esac
done
