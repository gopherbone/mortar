# The "Claude Code" launcher entry runs this in foot: attach to the
# persistent "claude" tmux session, creating it on first use. Closing the
# terminal only detaches; Claude Code keeps working. Over SSH the same
# session is `tmux -L mortar-ai attach -t claude`.
#
# Env: MORTAR_AI_SOCKET, MORTAR_AI_TMUX_CONF, MORTAR_AI_WORKDIR.
mkdir -p "$MORTAR_AI_WORKDIR"
exec tmux -L "$MORTAR_AI_SOCKET" -f "$MORTAR_AI_TMUX_CONF" \
  new-session -A -s claude -c "$MORTAR_AI_WORKDIR" mortar-claude-session
