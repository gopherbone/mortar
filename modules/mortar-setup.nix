# SPDX-License-Identifier: GPL-2.0-or-later
# Mortar first-boot setup and remote access (pkgs/mortar-setup):
#
#   mortar-setup-apply   boot: apply <card>/mortar.toml (host name, Wi-Fi for
#                        iwd, SSH keys, switches), then blank the Wi-Fi
#                        password in the file. Format: docs/setup-file.md.
#   mortar-setupd        pairing service for `mortar pair` (TCP 7330, usb0
#                        only, peers from 10.55.0.0/24 only), the pairing
#                        code, Wi-Fi through iwd, Bluetooth through BlueZ
#                        (it is BlueZ's pairing agent), and the control socket the
#                        screens use (/run/mortar-setup/control.sock, group
#                        mortar-setup: membership is root-equivalent).
#   mortar-setup-ui      first boot only: the setup screens on the panel,
#                        before the greeter, until setup is done or skipped.
#   sshd                 while "password login" is on and a code is
#                        showing, the code is root's SSH password over USB
#                        (Match LocalAddress 10.55.0.1 + pam_exec).
#
# mortar-shell's Settings > Wi-Fi, Bluetooth and Remote access open the same screens
# (modules/mortar-shell.nix passes the socket when this is enabled).
#
# Off by default: importing this module changes nothing until
# `mortar.setup.enable = true`. Needs modules/mortar-usb.nix.
{
  config,
  lib,
  pkgs,
  options,
  ...
}:

let
  inherit (lib) mkOption mkEnableOption types mkIf mkMerge;
  cfg = config.mortar.setup;
  exe = lib.getExe cfg.package;
  systemctl = "${config.systemd.package}/bin/systemctl";
  greeterOn = config.mortar ? greeter && config.mortar.greeter.enable;
  shellOn = config.mortar ? shell && config.mortar.shell.enable;

  daemonArgs = lib.escapeShellArgs (
    [
      "daemon"
      "--socket-group"
      "mortar-setup"
      "--port"
      (toString cfg.pairing.port)
      "--sshd-reload"
      "${systemctl} try-reload-or-restart sshd.service"
    ]
    ++ lib.concatMap (i: [ "--iface" i ]) cfg.pairing.interfaces
    ++ lib.concatMap (a: [ "--allow" a ]) cfg.pairing.allow
    ++ lib.optional (!cfg.sshPasswordWithCode) "--no-ssh-password"
  );

  # The first-boot screens run once per boot, only while setup is not done,
  # and never while a greeter session holds the display (e.g. when this is
  # first switched to on a running Brick).
  uiCondition = pkgs.writeShellScript "mortar-setup-ui-condition" ''
    [ -e /var/lib/mortar-setup/done ] && exit 1
    [ -e /run/mortar-setup/ui-started ] && exit 1
    ${systemctl} is-active --quiet greetd.service && exit 1
    exit 0
  '';
in
{
  options.mortar.setup = {
    enable = mkEnableOption "Mortar's first-boot setup, mortar.toml, `mortar pair` and the remote-access settings";

    package = mkOption {
      type = types.package;
      default = pkgs.callPackage ../pkgs/mortar-setup { };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/mortar-setup { }";
      description = "The mortar-setup package.";
    };

    ui.enable = mkOption {
      type = types.bool;
      default = true;
      description = ''
        Show the setup screens on the panel at boot until setup is finished
        (or skipped). With the greeter, it waits for them.
      '';
    };

    pairing = {
      port = mkOption {
        type = types.port;
        default = 7330;
        description = "TCP port of the pairing service.";
      };
      interfaces = mkOption {
        type = types.listOf types.str;
        default = [ "usb0" ];
        description = ''
          Interfaces the pairing service listens on (SO_BINDTODEVICE). The
          protocol is safe on an untrusted network (SPAKE2: one online guess
          per connection, 10 per code, then a button press on the Brick), but
          by default only the USB cable can pair.
        '';
      };
      allow = mkOption {
        type = types.listOf types.str;
        default = [ "10.55.0.0/24" ];
        description = "Peer address ranges (CIDR) allowed to pair or to use the code as SSH password.";
      };
    };

    sshPasswordWithCode = mkOption {
      type = types.bool;
      default = true;
      description = ''
        While "password login" is on (Settings > Remote access, or
        `[ssh] password_login` in mortar.toml) and a pairing code is on the
        screen, the code works as root's SSH password over USB, so plain
        `ssh root@mortar.local` / `ssh-copy-id` work. Off by itself once a
        key is added.
      '';
    };

    seedSetupFile = mkOption {
      type = types.bool;
      default = true;
      description = "Put a blank, commented mortar.toml on the SD image's boot partition.";
    };

    controlUsers = mkOption {
      type = types.listOf types.str;
      default = lib.optionals greeterOn (lib.filter (u: u != "root") (map (p: p.user) (lib.attrValues config.mortar.greeter.profiles)));
      defaultText = lib.literalMD "the greeter's profile users";
      description = ''
        Users that may use the setup screens from their session (group
        `mortar-setup`). They can see the pairing code, so this is
        root-equivalent: only the people holding the Brick.
      '';
    };
  };

  config = mkIf cfg.enable (mkMerge [
    {
      assertions = [
        {
          assertion = config.mortar.usb.enable;
          message = "mortar.setup needs mortar.usb (modules/mortar-usb.nix).";
        }
      ];

      environment.systemPackages = [ cfg.package ];
      users.groups.mortar-setup.members = cfg.controlUsers;

      systemd.tmpfiles.rules = [ "d /var/lib/mortar-setup 0700 root root - -" ];

      systemd.services.mortar-setup-apply = {
        description = "Mortar: apply mortar.toml from the boot partition";
        wantedBy = [ "multi-user.target" ];
        wants = [ "network-pre.target" ];
        before = [
          "network-pre.target"
          "systemd-networkd.service"
          "iwd.service"
          "sshd.service"
          "avahi-daemon.service"
          "shutdown.target"
        ];
        after = [ "local-fs.target" ];
        conflicts = [ "shutdown.target" ];
        unitConfig = {
          DefaultDependencies = false;
          RequiresMountsFor = [ "/boot" "/var/lib" ];
        };
        path = [ pkgs.iw config.systemd.package ];
        restartIfChanged = false;
        serviceConfig = {
          Type = "oneshot";
          RemainAfterExit = true;
          ExecStart = "${exe} apply";
        };
      };

      systemd.services.mortar-setupd = {
        description = "Mortar pairing service (USB) and setup state";
        wantedBy = [ "multi-user.target" ];
        wants = [ "mortar-setup-apply.service" ];
        after = [
          "mortar-setup-apply.service"
          "mortar-usb-gadget.service"
          "sshd.service"
          "iwd.service"
          "dbus.service"
        ];
        path = [ config.systemd.package ];
        serviceConfig = {
          ExecStart = "${exe} ${daemonArgs}";
          Restart = "always";
          RestartSec = "2s";
          RuntimeDirectory = "mortar-setup";
          RuntimeDirectoryPreserve = "yes";
        };
      };

      # Avahi publishes the system host name, which mortar.toml may set.
      services.avahi.settings.server.host-name = lib.mkForce "";
    }

    (mkIf cfg.sshPasswordWithCode {
      # mortar-setupd puts a Match block here while the code doubles as
      # root's password (sshd re-reads it on reload).
      services.openssh.extraConfig = lib.mkAfter ''
        Match all
        Include /run/mortar-setup/sshd/*.conf
      '';
      security.pam.services.sshd.rules.auth.mortar-pairing-code = {
        # before every other auth rule (autoOrderRules start at 10100)
        order = 10050;
        control = "sufficient";
        modulePath = "${config.security.pam.package}/lib/security/pam_exec.so";
        args = [
          "quiet"
          "expose_authtok"
          exe
          "check-password"
        ];
      };
    })

    (mkIf cfg.ui.enable {
      systemd.services.mortar-setup-ui = {
        description = "Mortar first-boot setup screens";
        wantedBy = [ "multi-user.target" ];
        wants = [ "mortar-setupd.service" ];
        after = [
          "mortar-setupd.service"
          "systemd-user-sessions.service"
          "systemd-logind.service"
        ];
        conflicts = [ "getty@tty1.service" ];
        restartIfChanged = false;
        environment.MORTAR_UI_THEME = if shellOn then config.mortar.shell.theme else "light";
        serviceConfig = {
          Type = "simple";
          ExecCondition = "${uiCondition}";
          ExecStartPre = "${pkgs.coreutils}/bin/touch /run/mortar-setup/ui-started /run/mortar-setup/ui.running";
          ExecStart = "${exe} ui";
          ExecStopPost = [
            "${pkgs.coreutils}/bin/rm -f /run/mortar-setup/ui.running"
          ]
          # without a greeter, give tty1 its console back
          ++ lib.optional (!greeterOn) "${systemctl} start --no-block getty@tty1.service";
          RuntimeDirectory = "mortar-setup";
          RuntimeDirectoryPreserve = "yes";
          StandardInput = "tty";
          StandardOutput = "journal";
          StandardError = "journal";
          TTYPath = "/dev/tty1";
          TTYReset = true;
          TTYVHangup = true;
        };
      };

      # On first boot the greeter waits for the setup screens to finish
      # (no start timeout: setup takes as long as it takes). Not with
      # After=mortar-setup-ui: that unit waits for mortar-setupd, sshd and the
      # network, and an ordering holds on every boot, also when the condition
      # skips the screens. So the shell would never start before them.
      systemd.services.greetd = mkIf greeterOn {
        after = [ "mortar-setup-apply.service" ];
        serviceConfig.ExecStartPre = [
          (pkgs.writeShellScript "mortar-greetd-first-boot" ''
            [ -e /var/lib/mortar-setup/done ] && exit 0
            ${systemctl} start mortar-setup-ui.service
            exec ${exe} wait-ui
          '')
        ];
        serviceConfig.TimeoutStartSec = "infinity";
      };
    })

    # A blank, commented setup file on new images.
    (lib.optionalAttrs (options ? sdImage) {
      sdImage.populateFirmwareCommands = mkIf cfg.seedSetupFile (
        lib.mkAfter ''
          cp ${cfg.package.template} ./firmware/mortar.toml
          chmod u+w ./firmware/mortar.toml
        ''
      );
    })
  ]);
}
