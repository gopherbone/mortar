# SPDX-License-Identifier: GPL-2.0-or-later
# Mortar AI: Claude Code on the Brick, inside the Mortar shell.
#
# Off by default: importing this module changes nothing until
# `mortar.ai.enable = true`. Claude Code is unfree; the flake's
# allowUnfreePredicate must allow "claude-code".
#
# Launcher entries (mortar.shell.apps):
#   Claude Code     foot running claude in a persistent tmux session
#                   ("claude" on tmux socket mortar-ai): switching away or
#                   closing the terminal keeps it running. First run walks
#                   through `claude auth login` with a QR code and a phone
#                   page to paste the code into (mortar-ai pair).
#   Claude (pad)    mortar-ai: a controller UI over `claude -p` stream-json;
#                   tool permission requests become cards, A allows, B denies.
#   Remote Control  starts/stops the mortar-claude-remote service:
#                   `claude remote-control` in its own tmux server, run as
#                   the user; attach shows its URL and (Y) QR code for the
#                   Claude app.
#
# Credentials: Claude Code keeps them in ~/.claude/.credentials.json (0600)
# in the user's home on the ext4 root file system, never on the FAT boot
# partition, and nothing here puts them in the Nix store or the repo.
#
# Memory: see pkgs/mortar-ai/DESIGN.md; `mortar-ai-mem` prints what to
# measure on the device.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  inherit (lib) mkOption mkEnableOption types;
  cfg = config.mortar.ai;
  home = config.users.users.${cfg.user}.home;

  mortar-ai = pkgs.callPackage ../pkgs/mortar-ai { mortar-shell = config.mortar.shell.package; };

  remoteArgs = lib.concatStringsSep " " (
    [ "--spawn" cfg.remoteControl.spawn ]
    ++ lib.optionals (cfg.remoteControl.permissionMode != null) [ "--permission-mode" cfg.remoteControl.permissionMode ]
  );

  scripts = pkgs.callPackage ../pkgs/mortar-ai/scripts.nix {
    claude = cfg.package;
    inherit mortar-ai remoteArgs;
    inherit (cfg) workDir;
    remoteName = cfg.remoteControl.name;
  };

  memScript = pkgs.writeShellApplication {
    name = "mortar-ai-mem";
    runtimeInputs = with pkgs; [ coreutils procps util-linux gawk ];
    text = ''
      # Memory picture for Mortar AI: run with Claude Code + the shell + an
      # app open (see pkgs/mortar-ai/DESIGN.md, "Memory").
      echo "== /proc/meminfo"
      grep -E '^(MemTotal|MemFree|MemAvailable|Cached|Shmem|SwapTotal|SwapFree|Committed_AS):' /proc/meminfo
      echo "== zram"
      zramctl 2>/dev/null || true
      echo "== pressure (avg10/60/300: % of time stalled on memory)"
      cat /proc/pressure/memory 2>/dev/null || echo "no PSI"
      echo "== top processes by RSS (MB): rss pss swap command"
      for p in /proc/[0-9]*; do
        r=$p/smaps_rollup
        [ -r "$r" ] || continue
        awk -v c="$(tr '\0' ' ' < "$p/cmdline" | cut -c1-60)" '
          /^Rss:/ {rss=$2} /^Pss:/ {pss=$2} /^Swap:/ {sw=$2}
          END { if (rss > 4096) printf "%6.0f %6.0f %6.0f  %s\n", rss/1024, pss/1024, sw/1024, c }' "$r" 2>/dev/null
      done | sort -rn | head -15
    '';
  };

  shellTerm = config.mortar.shell.terminal.command;
in
{
  options.mortar.ai = {
    enable = mkEnableOption "Claude Code on the Brick (launcher entries, tmux sessions, Remote Control)";

    pairPort = mkOption {
      type = types.port;
      default = 8471;
      description = ''
        TCP port of the sign-in page (`mortar-ai pair`) the phone opens from
        the QR code. Opened on wlan0 only. Nothing listens on it except
        while a sign-in is in progress, and the page sits behind a one-time
        128-bit token path.
      '';
    };

    package = mkOption {
      type = types.package;
      default = pkgs.claude-code;
      defaultText = lib.literalExpression "pkgs.claude-code";
      description = "Claude Code (unfree: allow \"claude-code\" in the flake's allowUnfreePredicate).";
    };

    user = mkOption {
      type = types.str;
      default = config.mortar.user.name or "root";
      defaultText = lib.literalExpression "config.mortar.user.name";
      description = ''
        Who runs Claude Code and owns its sign-in (~/.claude): the session
        user (modules/mortar-user.nix), whose apps the shell starts.
      '';
    };

    workDir = mkOption {
      type = types.str;
      default = "${home}/ai";
      defaultText = lib.literalExpression ''"''${home}/ai"'';
      description = ''
        Where Claude Code and Remote Control start. Not the home directory
        itself: Claude Code never saves folder trust for $HOME, so Remote
        Control would ask on every start.
      '';
    };

    controllerApp = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = "Add the controller UI (mortar-ai: A allows, B denies) to the launcher.";
      };
      model = mkOption {
        type = types.nullOr types.str;
        default = null;
        example = "sonnet";
        description = "Model for the controller UI (`claude --model`); null for Claude Code's default.";
      };
    };

    remoteControl = {
      enable = mkOption {
        type = types.bool;
        default = true;
        description = "Add the Remote Control launcher entry and its systemd service.";
      };
      name = mkOption {
        type = types.str;
        default = config.networking.hostName;
        defaultText = lib.literalExpression "config.networking.hostName";
        description = "Session name shown in claude.ai/code and the Claude app.";
      };
      spawn = mkOption {
        type = types.enum [ "same-dir" "worktree" "session" ];
        default = "same-dir";
        description = "`claude remote-control --spawn` (given explicitly so it never stops to ask).";
      };
      permissionMode = mkOption {
        type = types.nullOr (types.enum [ "acceptEdits" "auto" "bypassPermissions" "default" "dontAsk" "plan" ]);
        default = null;
        description = "`--permission-mode` for sessions spawned from the phone; null keeps Claude Code's default (ask).";
      };
      autoStart = mkOption {
        type = types.bool;
        default = false;
        description = "Start Remote Control at boot (it needs a signed-in user and Wi-Fi).";
      };
    };

    scripts = mkOption {
      type = types.package;
      readOnly = true;
      default = scripts;
      description = "The generated launcher scripts (mortar-claude, mortar-claude-remote, ...).";
    };
  };

  config = lib.mkIf cfg.enable (
    lib.mkMerge [
      {
        environment.variables.MORTAR_AI_PAIR_PORT = toString cfg.pairPort;
        networking.firewall.interfaces.wlan0.allowedTCPPorts = [ cfg.pairPort ];
      }
      {
        assertions = [
          {
            assertion = config.users.users ? ${cfg.user};
            message = "mortar.ai.user: user '${cfg.user}' does not exist.";
          }
          {
            assertion = !(lib.hasPrefix "/boot" home);
            message = "mortar.ai: ${cfg.user}'s home must be on the root file system (Claude Code keeps credentials there).";
          }
        ];

        environment.systemPackages = [
          cfg.package
          pkgs.tmux
          pkgs.git
          pkgs.foot.terminfo # tmux inside foot (TERM=foot)
          mortar-ai
          scripts
          memScript
        ];

        # The work directory, and ~/.claude private from the start.
        systemd.tmpfiles.rules = [
          "d ${cfg.workDir} 0700 ${cfg.user} - -"
          "d ${home}/.claude 0700 ${cfg.user} - -"
        ];

        mortar.shell.apps = {
          claude = {
            name = "Claude Code";
            description = "Terminal · keeps running";
            icon = "claude";
            gamepad = "keyboard";
            singleInstance = true;
            order = 6;
            command = shellTerm ++ [ "${scripts}/bin/mortar-claude" ];
          };
        }
        // lib.optionalAttrs cfg.controllerApp.enable {
          claude-pad = {
            name = "Claude";
            description = "A allows · B denies";
            icon = "claude";
            gamepad = "keyboard";
            singleInstance = true;
            order = 7;
            command = [ "${lib.getExe mortar-ai}" "run" "--cwd" cfg.workDir ]
              ++ lib.optionals (cfg.controllerApp.model != null) [ "--model" cfg.controllerApp.model ];
            env.MORTAR_AI_CLAUDE = lib.getExe cfg.package;
          };
        }
        // lib.optionalAttrs cfg.remoteControl.enable {
          claude-remote = {
            name = "Remote Control";
            description = "Claude from your phone";
            icon = "claude";
            gamepad = "keyboard";
            singleInstance = true;
            order = 8;
            command = shellTerm ++ [ "${scripts}/bin/mortar-claude-remote" ];
          };
        };
      }

      (lib.mkIf cfg.remoteControl.enable {
        # `claude remote-control` must keep running, and asks on a terminal
        # the first time (folder trust), so it runs in a tmux server of its
        # own; the launcher entry attaches to it. Stopping the unit kills the
        # tmux server and with it claude.
        systemd.services.mortar-claude-remote = {
          description = "Claude Code Remote Control (in tmux)";
          after = [ "network.target" ];
          wantedBy = lib.optional cfg.remoteControl.autoStart "multi-user.target";
          environment = {
            HOME = home;
            TMUX_TMPDIR = "/tmp";
            TERM = "tmux-256color";
          };
          serviceConfig = {
            Type = "forking";
            User = cfg.user;
            WorkingDirectory = cfg.workDir;
            ExecStart = "${pkgs.tmux}/bin/tmux -L mortar-remote -f ${scripts.tmuxRemoteConf} new-session -d -s remote -x 110 -y 36 ${scripts}/bin/mortar-claude-remote-run";
            ExecStop = "${pkgs.tmux}/bin/tmux -L mortar-remote kill-server";
            Restart = "no";
          };
        };
      })

      # A non-root session user may start/stop just this unit.
      (lib.mkIf (cfg.remoteControl.enable && cfg.user != "root") {
        security.polkit.enable = true;
        security.polkit.extraConfig = ''
          polkit.addRule(function(action, subject) {
            if (action.id == "org.freedesktop.systemd1.manage-units" &&
                action.lookup("unit") == "mortar-claude-remote.service" &&
                subject.user == "${cfg.user}") {
              return polkit.Result.YES;
            }
          });
        '';
      })
    ]
  );
}
