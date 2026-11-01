# The program in Remote Control's tmux pane (systemd unit
# mortar-claude-remote, started from the "Remote Control" launcher entry).
# `claude remote-control` prints the session URL and, on Space (the pad's
# Y), a QR code for the Claude app; it must keep running for the phone to
# reach the Brick.
#
# Env: MORTAR_AI_LIB, MORTAR_AI_WORKDIR, MORTAR_AI_REMOTE_NAME,
# MORTAR_AI_REMOTE_ARGS (extra flags, space separated).
# shellcheck source=/dev/null
. "$MORTAR_AI_LIB"

cd "$MORTAR_AI_WORKDIR" || exit 1
if ! signed_in; then
  heading "Remote Control: not signed in"
  printf '  Open "Claude Code" from the launcher first and sign in there.\n'
  printf '  Then start Remote Control again.\n'
  read -r -t 3600 _ || true
  exit 1
fi

# shellcheck disable=SC2086 # MORTAR_AI_REMOTE_ARGS is a list of flags
claude remote-control --name "$MORTAR_AI_REMOTE_NAME" ${MORTAR_AI_REMOTE_ARGS:-} && rc=0 || rc=$?
printf '\n  claude remote-control exited (status %s).\n' "$rc"
printf '  %s close (Remote Control stops)\n' "$(cap A)"
# keep the message on screen for whoever attaches next, for up to an hour
read -r -t 3600 _ || true
