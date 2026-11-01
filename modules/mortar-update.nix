# SPDX-License-Identifier: GPL-2.0-or-later
# Over-the-air updates (pkgs/mortar-update, docs/updates.md).
#
# CI (.github/workflows/build.yml) or `mortar publish` builds the system,
# pushes the closure, signed, to a private S3-compatible bucket (Cloudflare
# R2) used as a Nix binary cache, and then writes channels/<channel>.json.
# The Brick:
#
#   mortar-update-init        boot: moves `[update]` credentials from the
#                             card's mortar.toml to /var/lib/mortar-update
#                             (root, 0600), writes the first status
#   mortar-update             `mortar-update check`: manifest -> `nix copy
#                             --from s3://…` (signatures checked against
#                             publicKey) -> system profile ->
#                             `switch-to-configuration boot`
#   mortar-update-auto.timer  the same once a day, while Auto-check daily
#                             is on (Settings > Auto-check daily)
#
# mortar-shell's Settings shows the status (/run/mortar-update/status.json)
# and toasts "Update ready" when one is staged. Older generations stay in
# the boot menu (rollback: docs/updates.md).
#
# No secrets here: bucket name, endpoint and the public key only. The read
# credentials never enter the Nix store.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  inherit (lib) mkOption mkEnableOption types mkIf;
  cfg = config.mortar.update;
  exe = lib.getExe cfg.package;
  systemctl = "${config.systemd.package}/bin/systemctl";

  configFile = pkgs.writeText "mortar-update.json" (
    builtins.toJSON {
      inherit (cfg) bucket endpoint region channel autoCheck minBattery keepGenerations;
      system = pkgs.stdenv.hostPlatform.system;
      minFreeMib = cfg.minFreeMiB;
      cardFile = cfg.cardFile;
      curl = "${pkgs.curl}/bin/curl";
      nix = "${config.nix.package}/bin/nix";
      nixEnv = "${config.nix.package}/bin/nix-env";
      nixCollectGarbage = "${config.nix.package}/bin/nix-collect-garbage";
    }
  );

  service = args: {
    path = [ config.nix.package pkgs.coreutils config.systemd.package ];
    serviceConfig = {
      Type = "oneshot";
      ExecStart = "${exe} ${args}";
      # gentle on the SD card and the UI
      Nice = 10;
      IOSchedulingClass = "idle";
      TimeoutStartSec = "6h";
    };
  };
in
{
  options.mortar.update = {
    enable = mkEnableOption "over-the-air updates from a private Nix binary cache (docs/updates.md)";

    package = mkOption {
      type = types.package;
      default = pkgs.callPackage ../pkgs/mortar-update { };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/mortar-update { }";
      description = "The mortar-update package.";
    };

    bucket = mkOption {
      type = types.str;
      example = "mortar-cache";
      description = "Bucket name (private).";
    };

    endpoint = mkOption {
      type = types.str;
      example = "https://0123456789abcdef0123456789abcdef.r2.cloudflarestorage.com";
      description = "S3 endpoint (R2: https://<ACCOUNT_ID>.r2.cloudflarestorage.com).";
    };

    region = mkOption {
      type = types.str;
      default = "auto";
      description = "S3 region (`auto` for R2).";
    };

    channel = mkOption {
      type = types.str;
      default = "mortar";
      description = "Manifest name: channels/<channel>.json in the bucket.";
    };

    publicKey = mkOption {
      type = types.str;
      example = "mortar-1:3l5X0L…=";
      description = ''
        Public half of the publisher's Nix signing key (`nix key
        convert-secret-to-public`). Added to nix.settings.trusted-public-keys:
        paths copied from the bucket must carry its signature.
      '';
    };

    autoCheck = mkOption {
      type = types.bool;
      default = true;
      description = "Default for Settings > Auto-check daily (changed on the Brick, it sticks).";
    };

    minBattery = mkOption {
      type = types.ints.between 0 100;
      default = 30;
      description = "No download below this battery level (percent) unless charging.";
    };

    minFreeMiB = mkOption {
      type = types.ints.unsigned;
      default = 1024;
      description = "Free space wanted in /nix/store before a download; below it unreferenced paths are collected first.";
    };

    keepGenerations = mkOption {
      type = types.ints.unsigned;
      default = 5;
      description = "System generations kept when an update is staged (0: all). The boot menu shows boot.loader.generic-extlinux-compatible.configurationLimit of them.";
    };

    cardFile = mkOption {
      type = types.str;
      default = "/boot/mortar.toml";
      description = "The setup file on the card (its `[update]` section carries the read credentials).";
    };

    configFile = mkOption {
      type = types.path;
      readOnly = true;
      default = configFile;
      description = "The generated /etc/mortar-update/config.json.";
    };

    shell = mkOption {
      type = types.nullOr (types.attrsOf types.anything);
      default = null;
      internal = true;
      description = "What mortar-shell's Settings use (status file, commands); set when enabled.";
    };
  };

  config = mkIf cfg.enable {
    assertions = [
      {
        assertion = builtins.match "[^:]+:[A-Za-z0-9+/=]+" cfg.publicKey != null;
        message = "mortar.update.publicKey: want `name:base64` (nix key convert-secret-to-public).";
      }
    ];

    environment.etc."mortar-update/config.json".source = configFile;
    environment.systemPackages = [ cfg.package ];

    nix.settings.extra-trusted-public-keys = [ cfg.publicKey ];

    systemd.tmpfiles.rules = [ "d /var/lib/mortar-update 0755 root root -" ];

    # Before mortar-setup-apply: it hashes mortar.toml, and the secret is
    # blanked here first (so setup sees the file once, as it stays).
    systemd.services.mortar-update-init = {
      description = "Mortar updates: credentials from the card, first status";
      wantedBy = [ "multi-user.target" ];
      before = [ "mortar-setup-apply.service" ];
      after = [ "local-fs.target" ];
      unitConfig.RequiresMountsFor = [ "/boot" "/var/lib" ];
      serviceConfig = {
        Type = "oneshot";
        RemainAfterExit = true;
        ExecStart = "${exe} init";
      };
    };

    # `mortar-update check`: Settings > Updates > Check now starts this.
    systemd.services.mortar-update = service "check" // {
      description = "Mortar updates: check, download and stage";
      after = [ "network.target" "mortar-update-init.service" ];
    };

    systemd.services.mortar-update-auto = service "check --auto" // {
      description = "Mortar updates: daily check";
      after = [ "network.target" "mortar-update-init.service" ];
    };
    systemd.timers.mortar-update-auto = {
      wantedBy = [ "timers.target" ];
      timerConfig = {
        OnBootSec = "20min";
        OnCalendar = "daily";
        Persistent = true;
        RandomizedDelaySec = "1h";
      };
    };

    # Settings > Auto-check daily: `systemctl start mortar-update-auto@on`
    systemd.services."mortar-update-auto@" = {
      description = "Mortar updates: auto-check %i";
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${exe} auto %i";
      };
    };

    # The session user (modules/mortar-user.nix) and other members of the
    # mortar-update group may start these units.
    users.groups.mortar-update = { };
    security.polkit.extraConfig = lib.mkIf config.security.polkit.enable ''
      polkit.addRule(function(action, subject) {
        if (action.id == "org.freedesktop.systemd1.manage-units" &&
            subject.isInGroup("mortar-update")) {
          var unit = action.lookup("unit");
          if (unit == "mortar-update.service" ||
              unit == "mortar-update-auto@on.service" ||
              unit == "mortar-update-auto@off.service") {
            return polkit.Result.YES;
          }
        }
      });
    '';

    # for the shell (modules/mortar-shell.nix)
    mortar.update.shell = {
      status = "/run/mortar-update/status.json";
      check = [ systemctl "start" "--no-block" "mortar-update.service" ];
      auto = [ systemctl "start" "--no-block" "mortar-update-auto@{on}.service" ];
    };
  };
}
