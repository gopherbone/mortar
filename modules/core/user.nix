# SPDX-License-Identifier: GPL-2.0-or-later
# The person holding the Brick: one normal account that runs the greeter
# sessions (the shell, its apps, Claude Code, PipeWire). Root stays for the
# consoles that need physical access and for SSH administration (deploys).
#
#   - No password, like root (base.nix). The greeter logs in without
#     a prompt (pam_succeed_if, greeter.nix); SSH is key-only, from
#     <card>/ssh-keys/<name> (optional; root's keys stay in ssh-keys/root).
#   - Groups for the hardware it drives directly: video (backlight, fbdev,
#     DRM), render, input (rumble, pads), uinput (per-app virtual pads),
#     audio, dialout (the M8's serial port), systemd-journal (the Remote
#     Control screen shows its unit's log).
#   - What used to need root goes through polkit (below): time zone
#     (timedated), restart/power off/sleep (logind, also while root has
#     SSH sessions open). Wi-Fi and remote access go through mortar-setupd's
#     control socket (group mortar-setup, setup.nix), Bluetooth
#     through mortar-setupd too, the backlight through a udev rule
#     (shell.nix).
#
# Moving from the root-session Brick: the first boot with this module copies
# the session data root collected (`migrateFromRoot`: Claude Code's sign-in
# and history, app data, PICO-8 carts, screenshots, ...) into the new home,
# fixes Claude Code's project paths, and parks each original under
# /root/.pre-brick-<date>/ (nothing is deleted; remove that folder once the
# new home is known good). Everything else stays in /root.
{ config, lib, pkgs, ... }:

let
  inherit (lib) mkOption types;
  cfg = config.mortar.user;
  home = cfg.home;
  group = config.users.users.${cfg.name}.group;
  stamp = "/var/lib/mortar-user/migrated-from-root";

  migrate = pkgs.writeShellApplication {
    name = "mortar-user-migrate";
    runtimeInputs = with pkgs; [ coreutils findutils gnused jq ];
    text = ''
      # Move the root session's data to ${cfg.name} (once; see the header of this file).
      # (MORTAR_MIGRATE_* only for the build-time test.)
      user=''${MORTAR_MIGRATE_USER:-${lib.escapeShellArg cfg.name}}
      group=''${MORTAR_MIGRATE_GROUP:-${lib.escapeShellArg group}}
      home=''${MORTAR_MIGRATE_HOME:-${lib.escapeShellArg home}}
      from=''${MORTAR_MIGRATE_FROM:-/root}
      stamp=''${MORTAR_MIGRATE_STAMP:-${stamp}}
      play=''${MORTAR_MIGRATE_PLAY:-${lib.escapeShellArg (if config.mortar.play.enable or false then config.mortar.play.dataDir else "")}}
      keep=$from/.pre-brick-$(date +%Y%m%d-%H%M%S)
      moved=0

      # parents below $home are created as the user
      mkparents() {
        local d=$1 todo=()
        while [ "$d" != "$home" ] && [ ! -e "$d" ]; do todo=("$d" "''${todo[@]}"); d=$(dirname "$d"); done
        for d in "''${todo[@]}"; do install -d -o "$user" -g "$group" -m 0755 "$d"; done
      }

      for rel in ${lib.escapeShellArgs cfg.migrateFromRoot}; do
        src=$from/$rel dst=$home/$rel
        [ -e "$src" ] || [ -L "$src" ] || continue
        # tmpfiles may already have made an empty directory (~/.claude, ~/ai)
        if [ -d "$dst" ] && [ ! -L "$dst" ] && [ -z "$(ls -A "$dst")" ]; then rmdir "$dst"; fi
        if [ -e "$dst" ] || [ -L "$dst" ]; then
          echo "keeping $src: $dst exists"
          continue
        fi
        mkparents "$(dirname "$dst")"
        # copy, then park the original (never deleted) until the new home
        # is known good: $from/.pre-brick-<date>/<path>
        cp -a --no-target-directory "$src" "$dst"
        chown -R --no-dereference "$user:$group" "$dst"
        mkdir -p "$(dirname "$keep/$rel")"
        mv -T "$src" "$keep/$rel"
        echo "copied $src (original kept in $keep)"
        moved=$((moved + 1))
      done

      # Claude Code keys folder trust and history by absolute path.
      cj=$home/.claude.json
      if [ -f "$cj" ]; then
        tmp=$(mktemp "$cj.XXXXXX")
        if jq --arg h "$home" '
          def fix: if . == "/root" then $h elif startswith("/root/") then $h + ltrimstr("/root") else . end;
          walk(if type == "string" then fix else . end)
          | if (.projects | type) == "object" then .projects |= with_entries(.key |= fix) else . end
        ' "$cj" > "$tmp"; then
          mv "$tmp" "$cj"
        else
          echo "could not rewrite paths in $cj"
          rm -f "$tmp"
        fi
        chown "$user:$group" "$cj"
        chmod 0600 "$cj"
      fi
      enc=$(printf %s "$home" | tr / -)
      if [ -d "$home/.claude/projects" ]; then
        for d in "$home"/.claude/projects/-root "$home"/.claude/projects/-root-*; do
          [ -d "$d" ] || continue
          new=$home/.claude/projects/$enc''${d#"$home"/.claude/projects/-root}
          [ -e "$new" ] || mv -T "$d" "$new"
        done
        find "$home/.claude/projects" -name '*.jsonl' -type f -exec \
          sed -i -e 's#"cwd":"/root/#"cwd":"'"$home"'/#g' -e 's#"cwd":"/root"#"cwd":"'"$home"'"#g' {} +
      fi

      # emulator saves, states and recents written by the root session
      if [ -n "$play" ]; then
        for d in saves states tmp recent.json; do
          if [ -e "$play/$d" ]; then chown -R --no-dereference "$user:$group" "$play/$d"; fi
        done
      fi

      mkdir -p "$(dirname "$stamp")"
      echo "$moved" > "$stamp"
      echo "migration done ($moved copied)"
    '';
  };

  # sshd reads AuthorizedKeysFile with the user's rights, and the FAT
  # partition is root-only (umask 0077), so the user's card file goes
  # through this (run as root, only for the session user).
  keysCommand = pkgs.writeShellScript "mortar-user-keys" ''
    [ "$1" = ${lib.escapeShellArg cfg.name} ] || exit 0
    f=/boot/ssh-keys/${cfg.name}
    [ -f "$f" ] && exec ${pkgs.coreutils}/bin/head -c 65536 "$f"
    exit 0
  '';
in
{
  options.mortar.user = {
    name = mkOption {
      type = types.str;
      default = "brick";
      description = "The session user: greeter profile, shell, apps, Claude Code.";
    };
    home = mkOption {
      type = types.str;
      default = "/home/${cfg.name}";
      defaultText = lib.literalExpression ''"/home/''${config.mortar.user.name}"'';
      description = "Its home directory; app data lives under it.";
    };
    uid = mkOption {
      type = types.int;
      default = 1000;
      description = "Its uid (fixed, so /run/user/<uid> is predictable).";
    };
    extraGroups = mkOption {
      type = types.listOf types.str;
      default = [ ];
      description = "More groups for the session user.";
    };
    migrateFromRoot = mkOption {
      type = types.listOf types.str;
      default = [
        ".claude"
        ".claude.json"
        ".claude.json.backup"
        "ai"
        ".lexaloffle"
        ".local/share/m8c"
        ".local/share/ChooChooTracker"
        ".local/share/puzzlescript"
        ".local/share/mortar-shell"
        ".local/share/mortar-play"
        ".local/share/mortar-ai"
        ".local/state/mortar-shell"
        ".local/state/mortar-ai"
        ".config/foot"
        "Pictures"
        "Games"
        ".bash_history"
      ];
      description = ''
        Paths under /root (the old session home) copied into the user's home
        on the first boot with this module; the originals are kept in
        /root/.pre-brick-<date>/. A path whose destination already holds
        something stays where it is. Remove
        /var/lib/mortar-user/migrated-from-root to run it again.
      '';
    };
  };

  config = {
    users.users.${cfg.name} = {
      isNormalUser = true;
      inherit (cfg) uid home;
      description = "Mortar";
      hashedPassword = "!"; # no password, like root
      extraGroups = [ "video" "render" "input" "uinput" "audio" "dialout" "systemd-journal" ]
        # real-time limits for PipeWire (pam_limits, set by the pipewire module)
        ++ lib.optional config.services.pipewire.enable "pipewire"
        # Settings > Updates: check now, auto-check (polkit rule in update.nix)
        ++ lib.optional (config.mortar.update.enable or false) "mortar-update"
        ++ cfg.extraGroups;
    };
    users.groups.uinput = { };

    # the user's own key, if <card>/ssh-keys/<name> exists
    # (a copy in /etc: sshd refuses commands under the group-writable store)
    environment.etc."ssh/mortar-user-keys" = {
      source = keysCommand;
      mode = "0755";
    };
    services.openssh.authorizedKeysCommand = "/etc/ssh/mortar-user-keys %u";
    services.openssh.authorizedKeysCommandUser = "root";

    security.polkit.enable = true;
    security.polkit.extraConfig = ''
      // Mortar: the session user may set the time zone and power the Brick
      // off, restart or sleep it (also while root is logged in over SSH).
      polkit.addRule(function(action, subject) {
        if (subject.user != "${cfg.name}") return polkit.Result.NOT_HANDLED;
        var id = action.id;
        if (id == "org.freedesktop.timedate1.set-timezone") return polkit.Result.YES;
        if (/^org\.freedesktop\.login1\.(power-off|reboot|suspend|halt)(-multiple-sessions|-ignore-inhibit)?$/.test(id))
          return polkit.Result.YES;
        return polkit.Result.NOT_HANDLED;
      });
    '';

    system.build.mortarUserMigrate = migrate;

    systemd.services.mortar-user-migrate = {
      description = "Mortar: copy the root session's data to ${cfg.name}";
      wantedBy = [ "multi-user.target" ];
      before = [ "greetd.service" "display-manager.service" "systemd-user-sessions.service" "mortar-claude-remote.service" ];
      after = [ "local-fs.target" "systemd-tmpfiles-setup.service" ];
      unitConfig = {
        ConditionPathExists = "!${stamp}";
        RequiresMountsFor = [ "/root" home "/var/lib" ];
      };
      restartIfChanged = false;
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = lib.getExe migrate;
      };
    };
  };
}
