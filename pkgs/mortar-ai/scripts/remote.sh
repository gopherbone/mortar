# The "Remote Control" launcher entry (in foot): start or stop the
# mortar-claude-remote service and show its screen (URL, QR code on Y).
#
# Env: MORTAR_AI_LIB, MORTAR_AI_REMOTE_UNIT, MORTAR_AI_REMOTE_SOCKET,
# MORTAR_AI_REMOTE_NAME, MORTAR_AI_WORKDIR.
# shellcheck source=/dev/null
. "$MORTAR_AI_LIB"

unit=$MORTAR_AI_REMOTE_UNIT
rtmux() { tmux -L "$MORTAR_AI_REMOTE_SOCKET" "$@"; }

running() { systemctl is-active --quiet "$unit" 2>/dev/null; }

show() {
  # wait for the service's tmux session, then attach; R1 comes back here
  for _ in $(seq 50); do
    rtmux has-session -t remote 2>/dev/null && break
    sleep 0.2
  done
  if rtmux has-session -t remote 2>/dev/null; then
    rtmux attach -t remote || true
  else
    printf '\n  Remote Control did not start. Last log lines:\n\n'
    journalctl -u "$unit" -n 8 --no-pager -o cat 2>/dev/null | sed 's/^/    /' || true
    printf '\n  %s back\n' "$(cap A)"
    read_button >/dev/null
  fi
}

while true; do
  heading "Remote Control"
  cat <<EOF
  Drive Claude Code on this Brick from the Claude app on your phone or
  from claude.ai/code. Claude runs here, in $(dim "$MORTAR_AI_WORKDIR");
  the phone only shows the session. The Brick must stay on and online.

EOF
  if running; then
    printf '  Status: %s  as "%s"\n\n' "$(bold running)" "$MORTAR_AI_REMOTE_NAME"
    printf '  %s show the link and QR code (Y in there toggles the QR, R1 back)\n' "$(cap A)"
    printf '  %s stop Remote Control\n' "$(cap X)"
  else
    printf '  Status: %s\n\n' "$(dim stopped)"
    if signed_in; then
      printf '  %s start Remote Control\n' "$(cap A)"
    else
      printf '  Not signed in yet: open %s from the launcher first.\n' "$(bold 'Claude Code')"
    fi
  fi
  if running; then
    printf '  %s close (Remote Control keeps running)\n\n' "$(cap B)"
  else
    printf '  %s close\n\n' "$(cap B)"
  fi
  printf '  %s\n' "$(dim 'First time in this folder? Claude asks to trust it: START, type y, Enter.')"
  case "$(read_button)" in
    a)
      if running; then
        show
      elif signed_in; then
        systemctl start "$unit" || true
        show
      fi
      ;;
    x)
      if running; then
        systemctl stop "$unit" || true
      fi
      ;;
    b | esc | eof) exit 0 ;;
  esac
done
