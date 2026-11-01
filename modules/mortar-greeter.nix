# SPDX-License-Identifier: GPL-2.0-or-later
# Mortar greeter: greetd + mortar-ui (pkgs/mortar-ui). Boots to a
# controller-driven login screen: profiles on the left (L1/R1), sessions on
# the right (D-pad), A starts. The last session per profile is remembered in
# stateDir, so a normal boot is one press.
#
# Off by default: importing this module changes nothing until
# `mortar.greeter.enable = true`.
#
# Login: Mortar accounts have no passwords. Profiles with
# `passwordless = true` get a pam_succeed_if rule in the greetd PAM stack, so
# greetd's create_session succeeds without a prompt (only through greetd; SSH
# and other PAM services are unaffected). For other profiles PAM decides; any
# prompt it sends (e.g. a future PIN module) is answered on the greeter's PIN
# pad.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  inherit (lib) mkOption mkEnableOption types;
  cfg = config.mortar.greeter;

  icons = [ "wifi" "bat" "usb" "cable" "file" "skip" "term" "folder" "pad" "gear" "link" "check" "lock" "power" "claude" ];

  sessionType = types.submodule (
    { name, ... }:
    {
      options = {
        name = mkOption {
          type = types.str;
          default = name;
          description = "Name shown on the greeter.";
        };
        description = mkOption {
          type = types.str;
          default = "";
          example = "Fullscreen, controller";
          description = "Second line under the name.";
        };
        icon = mkOption {
          type = types.enum icons;
          default = "term";
          description = "Line icon from the Mortar UI set.";
        };
        command = mkOption {
          type = types.either types.str (types.listOf types.str);
          example = lib.literalExpression ''"''${pkgs.cage}/bin/cage -- ''${pkgs.foot}/bin/foot"'';
          description = ''
            What the session runs, as a shell command string or an argv list.
            greetd runs it as the user on VT1 after sourcing /etc/profile.
          '';
        };
        env = mkOption {
          type = types.attrsOf types.str;
          default = { };
          description = "Extra environment. MORTAR_SESSION and XDG_SESSION_DESKTOP are always set to the session id.";
        };
        needsWifi = mkOption {
          type = types.bool;
          default = false;
          description = "Mark the session as needing Wi-Fi; starting it offline asks for a second press.";
        };
        order = mkOption {
          type = types.int;
          default = 100;
          description = "Sort key (then id) for the session list.";
        };
      };
    }
  );

  profileType = types.submodule (
    { name, ... }:
    {
      options = {
        user = mkOption {
          type = types.str;
          default = name;
          description = "Unix account this profile logs in as.";
        };
        displayName = mkOption {
          type = types.nullOr types.str;
          default = null;
          description = "Name shown on the greeter (default: the user name).";
        };
        sessions = mkOption {
          type = types.nullOr (types.listOf types.str);
          default = null;
          example = [ "games" ];
          description = "Session ids this profile may start (null = all).";
        };
        passwordless = mkOption {
          type = types.bool;
          default = true;
          description = "Log in through the greeter without any PAM prompt.";
        };
        pin = mkOption {
          type = types.bool;
          default = false;
          description = ''
            Display hint: show "PIN" on the profile. Set this together with
            `passwordless = false` once a PIN PAM module exists; the greeter
            answers whatever PAM asks on its PIN pad.
          '';
        };
        order = mkOption {
          type = types.int;
          default = 100;
          description = "Sort key (then user) for the profile list.";
        };
      };
    }
  );

  byOrder = key: lib.sort (a: b: if a.order == b.order then a.${key} < b.${key} else a.order < b.order);

  sessions = byOrder "id" (lib.mapAttrsToList (id: s: s // { inherit id; }) cfg.sessions);
  profiles = byOrder "user" (lib.attrValues cfg.profiles);

  configFile = pkgs.writeText "mortar-greeter.json" (
    builtins.toJSON {
      profiles = map (p: {
        inherit (p) user sessions;
        name = p.displayName;
        auth = if p.pin then "pin" else "none";
      }) profiles;
      sessions = map (s: {
        inherit (s) id name description icon env needsWifi;
        # greetd joins cmd with spaces and runs it with `sh -c "exec <cmd>"`,
        # so every word must already be shell-quoted. (Unquoted, a string
        # command "exec foo" became `bash -c exec foo`: a no-op session.)
        command = map lib.escapeShellArg (
          if builtins.isString s.command then [ "${pkgs.runtimeShell}" "-c" s.command ] else s.command
        );
      }) sessions;
      stateDir = cfg.stateDir;
      switchDir = "/run/mortar-greeter";
    }
  );

  passwordlessUsers = map (p: p.user) (lib.filter (p: p.passwordless) profiles);

  normalUsers = lib.attrNames (lib.filterAttrs (_: u: u.isNormalUser) config.users.users);
in
{
  options.mortar.greeter = {
    enable = mkEnableOption "the Mortar greeter (greetd + mortar-ui) on the panel";

    package = mkOption {
      type = types.package;
      default = pkgs.callPackage ../pkgs/mortar-ui { };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/mortar-ui { }";
      description = "The mortar-ui package.";
    };

    profiles = mkOption {
      type = types.attrsOf profileType;
      default = lib.genAttrs (if normalUsers == [ ] then [ "root" ] else normalUsers) (_: { });
      defaultText = lib.literalMD "every `isNormalUser` account, or `root` if there are none";
      example = lib.literalExpression ''
        {
          alex = { };
          guest = { sessions = [ "games" ]; order = 200; };
        }
      '';
      description = "Profiles on the left of the greeter, keyed by user name.";
    };

    sessions = mkOption {
      type = types.attrsOf sessionType;
      default = {
        terminal = {
          name = "Terminal";
          description = "Console";
          icon = "term";
          command = "exec ${pkgs.bashInteractive}/bin/bash --login";
        };
      };
      defaultText = lib.literalMD "a Terminal session (login shell on the console)";
      example = lib.literalExpression ''
        {
          games = { name = "Games"; description = "Fullscreen, controller"; icon = "pad"; order = 10; command = "..."; };
          desktop = { name = "Desktop"; description = "Wayland, keyboard"; icon = "folder"; order = 20;
                      command = "''${pkgs.cage}/bin/cage -- ''${pkgs.foot}/bin/foot"; };
          ai = { name = "AI"; description = "Needs Wi-Fi"; icon = "link"; needsWifi = true; command = "..."; };
        }
      '';
      description = "Sessions on the right of the greeter, keyed by id.";
    };

    stateDir = mkOption {
      type = types.str;
      default = "/var/lib/mortar-greeter";
      description = "Where the greeter remembers the last profile and each profile's last session.";
    };

    configFile = mkOption {
      type = types.path;
      readOnly = true;
      default = configFile;
      description = "The generated greeter config (also at /etc/mortar-greeter/config.json).";
    };
  };

  config = lib.mkIf cfg.enable {
    assertions = [
      {
        assertion = sessions != [ ];
        message = "mortar.greeter.sessions must define at least one session.";
      }
      {
        assertion = profiles != [ ];
        message = "mortar.greeter.profiles must define at least one profile.";
      }
    ]
    ++ map (p: {
      assertion = config.users.users ? ${p.user};
      message = "mortar.greeter.profiles: user '${p.user}' does not exist in users.users.";
    }) profiles
    ++ lib.concatMap (
      p:
      map (id: {
        assertion = cfg.sessions ? ${id};
        message = "mortar.greeter.profiles.${p.user}.sessions: unknown session '${id}'.";
      }) (if p.sessions == null then [ ] else p.sessions)
    ) profiles;

    services.greetd = {
      enable = true;
      settings.default_session = {
        command = "${lib.getExe cfg.package} greeter --config ${configFile}";
        user = "greeter";
      };
    };

    # Passwordless profiles: succeed in the greetd auth stack before the
    # `login` substack (autoOrderRules numbers that one 10100).
    security.pam.services.greetd.rules.auth.mortar-passwordless = lib.mkIf (passwordlessUsers != [ ]) {
      order = 10000;
      control = "sufficient";
      modulePath = "${config.security.pam.package}/lib/security/pam_succeed_if.so";
      args = [
        "quiet"
        "user"
        "in"
        (lib.concatStringsSep ":" passwordlessUsers)
      ];
    };

    # The greeter draws on /dev/dri/card* and reads /dev/input/event*.
    # logind's seat ACLs normally cover this; the groups make it robust.
    users.users.greeter.extraGroups = [
      "video"
      "input"
    ];

    systemd.tmpfiles.rules = [
      "d ${cfg.stateDir} 0750 greeter greeter - -"
      # quick-menu "switch session" requests: anyone may drop a file, only
      # its owner's request is honoured (the greeter checks the uid)
      "d /run/mortar-greeter 1733 greeter greeter - -"
    ];

    # Brightness from the quick menu (video group may write the backlight).
    services.udev.extraRules = ''
      ACTION=="add", SUBSYSTEM=="backlight", RUN+="${pkgs.coreutils}/bin/chgrp video /sys/class/backlight/%k/brightness", RUN+="${pkgs.coreutils}/bin/chmod g+w /sys/class/backlight/%k/brightness"
    '';

    # `mortar-ui quick-menu` inside sessions reads the same config.
    environment.etc."mortar-greeter/config.json".source = configFile;
    environment.systemPackages = [ cfg.package ];
  };
}
