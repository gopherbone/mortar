# SPDX-License-Identifier: GPL-2.0-or-later
# The terminal side of Mortar AI (scripts/*.sh), built with the paths and
# names the NixOS module (modules/packs/ai.nix) or a test chooses.
#
#   mortar-claude                foot runs this: attach to (or create) the
#                                persistent "claude" tmux session
#   mortar-claude-session        inside it: sign-in guide, claude, menu
#   mortar-claude-remote         foot runs this: Remote Control menu
#   mortar-claude-remote-run     inside the remote service's tmux session
{
  lib,
  writeShellApplication,
  writeText,
  symlinkJoin,
  tmux,
  coreutils,
  gnugrep,
  gnused,
  systemd,
  bashInteractive,
  git,
  claude,
  mortar-ai,
  workDir,
  socket ? "mortar-ai",
  remoteSocket ? "mortar-remote",
  remoteUnit ? "mortar-claude-remote",
  remoteName ? "Mortar",
  remoteArgs ? "",
}:

let
  tmuxConf = ./scripts/tmux.conf;
  tmuxRemoteConf = writeText "mortar-ai-tmux-remote.conf" (
    lib.replaceStrings [ "@baseConf@" ] [ "${tmuxConf}" ] (builtins.readFile ./scripts/tmux-remote.conf)
  );
  runtimeEnv = {
    MORTAR_AI_LIB = "${./scripts/common.sh}";
    MORTAR_AI_SOCKET = socket;
    MORTAR_AI_TMUX_CONF = "${tmuxConf}";
    MORTAR_AI_WORKDIR = workDir;
    MORTAR_AI_REMOTE_SOCKET = remoteSocket;
    MORTAR_AI_REMOTE_UNIT = remoteUnit;
    MORTAR_AI_REMOTE_NAME = remoteName;
    MORTAR_AI_REMOTE_ARGS = remoteArgs;
  };
  mk =
    name: file: extra:
    writeShellApplication {
      inherit name runtimeEnv;
      runtimeInputs = [
        claude
        tmux
        coreutils
        gnugrep
        gnused
        systemd
        bashInteractive
        git
        mortar-ai
      ] ++ extra;
      text = builtins.readFile file;
    };
  session = mk "mortar-claude-session" ./scripts/claude-session.sh [ ];
  attach = mk "mortar-claude" ./scripts/claude-attach.sh [ session ];
  remoteRun = mk "mortar-claude-remote-run" ./scripts/remote-run.sh [ ];
  remote = mk "mortar-claude-remote" ./scripts/remote.sh [ ];
in
symlinkJoin {
  name = "mortar-ai-scripts";
  paths = [ session attach remote remoteRun ];
  passthru = { inherit tmuxConf tmuxRemoteConf session attach remote remoteRun; };
}
