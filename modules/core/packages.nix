# SPDX-License-Identifier: GPL-2.0-or-later
# Packages (pkgs/mortar-packages, docs/packages.md): installing more from the Brick,
# with no system rebuild or restart.
#
#   mortar-packages.socket     only the session user can reach it
#   mortar-packages.service    root, started by the socket, exits when idle. Takes a
#                              repository and a catalog ID, never a path or URL:
#                              index (signature, channel, device, date) -> battery and
#                              room -> `nix-store --realise` with that source's URL
#                              and key -> `nix-env --set` on the app's own profile,
#                              which the shell notices
#   mortar-packages-auto       official packages, right after the daily system check,
#                              while Auto-check daily is on
#
# Repositories are set here and nowhere else: nothing on the socket can add one.
# Mortar's own comes from `mortar.update`.
{
  config,
  lib,
  pkgs,
  ...
}:

let
  inherit (lib) mkOption types mkIf;
  cfg = config.mortar.packages;
  update = config.mortar.update;
  exe = "${cfg.package}/bin/mortar-packagesd";
  toolsBin = "/var/lib/mortar-packages/tools/bin";

  # An S3-mode system reads a private bucket with credentials, and has no public
  # index to read: it follows nothing here until it's given a repository.
  mortarRepo = {
    id = "mortar";
    name = "Mortar";
    url = update.baseUrl;
    publicKeys = update.trustedKeys;
    kind = "nix";
    channel = update.channel;
    upstreams = map (u: { inherit (u) url publicKey; }) update.upstreams;
    official = true;
  };

  configFile = pkgs.writeText "mortar-packages.json" (
    builtins.toJSON {
      device = config.mortar.device.id;
      system = pkgs.stdenv.hostPlatform.system;
      repositories = cfg.repositories;
      minBattery = update.minBattery;
      minFreeMib = update.minFreeMiB;
      autoCheck = update.autoCheck;
      idleSeconds = cfg.idleSeconds;
      inherit toolsBin;
      # the folder the shell watches
      appProfiles = if config.mortar.shell.appProfiles or null != null then config.mortar.shell.appProfiles else "/nix/var/nix/profiles/apps";
      curl = "${pkgs.curl}/bin/curl";
      nixEnv = "${config.nix.package}/bin/nix-env";
      nixStore = "${config.nix.package}/bin/nix-store";
      nixCollectGarbage = "${config.nix.package}/bin/nix-collect-garbage";
    }
  );

  # A unit's environment has none of these, and both curl and Nix check TLS.
  tls = {
    CURL_CA_BUNDLE = config.security.pki.caBundle;
    NIX_SSL_CERT_FILE = config.security.pki.caBundle;
    SSL_CERT_FILE = config.security.pki.caBundle;
  };
in
{
  options.mortar.packages = {
    enable = mkOption {
      type = types.bool;
      default = (config.mortar.shell.enable or false) && (config.mortar.shell.packages.enable or false);
      defaultText = lib.literalExpression "config.mortar.shell.enable && config.mortar.shell.packages.enable";
      description = "The service behind the Packages app.";
    };

    package = mkOption {
      type = types.package;
      default = pkgs.callPackage ../../pkgs/mortar-packages { };
      defaultText = lib.literalExpression "pkgs.callPackage ../../pkgs/mortar-packages { }";
      description = "The mortar-packages package.";
    };

    repositories = mkOption {
      type = types.listOf (types.submodule {
        options = {
          id = mkOption { type = types.str; description = "What requests call it."; };
          name = mkOption { type = types.str; description = "What the screens call it."; };
          url = mkOption { type = types.str; description = "<url>/channels/<channel>/catalog.json, and a binary cache."; };
          publicKeys = mkOption { type = types.listOf types.str; description = "What signs its index and its packages."; };
          kind = mkOption { type = types.enum [ "nix" "bundle" ]; default = "nix"; description = "What it offers. Bundle repositories aren't read yet."; };
          channel = mkOption { type = types.str; description = "Which of its channels."; };
          upstreams = mkOption {
            type = types.listOf (types.submodule {
              options = {
                url = mkOption { type = types.str; };
                publicKey = mkOption { type = types.str; };
              };
            });
            default = [ ];
            description = "Where the rest of a closure comes from. Their keys count for store paths, never for an index.";
          };
          official = mkOption { type = types.bool; default = false; description = "Its packages update on the daily check. Others are only offered."; };
        };
      });
      default = lib.optional (update.enable && update.baseUrl != null) mortarRepo;
      defaultText = "Mortar's, from mortar.update, in HTTPS mode";
      description = ''
        What this system reads. There's no other way to add one: not on the socket, and
        not from an app. Adding a Nix repository trusts it with your whole system.
      '';
    };

    idleSeconds = mkOption {
      type = types.ints.positive;
      default = 20;
      description = "The service exits after this long with nothing to do.";
    };
  };

  config = mkIf cfg.enable {
    environment.etc."mortar-packages/config.json".source = configFile;
    environment.systemPackages = [ cfg.package ];

    users.groups.mortar-packages = { };

    systemd.tmpfiles.rules = [ "d /var/lib/mortar-packages 0755 root root -" ];

    systemd.sockets.mortar-packages = {
      description = "Packages: the socket the app reaches the service on";
      wantedBy = [ "sockets.target" ];
      socketConfig = {
        ListenStream = "/run/mortar-packages/socket";
        SocketUser = "root";
        SocketGroup = "mortar-packages";
        SocketMode = "0660";
      };
    };

    systemd.services.mortar-packages = {
      description = "Packages: install, update and remove";
      path = [ config.nix.package pkgs.coreutils ];
      environment = tls;
      restartIfChanged = false;
      serviceConfig = {
        ExecStart = exe;
        # gentle on the SD card and the UI
        Nice = 10;
        IOSchedulingClass = "idle";
      };
    };

    # The daily check's other half: mortar-update-auto starts it, after the system.
    systemd.services.mortar-packages-auto = mkIf update.enable {
      description = "Packages: the daily check";
      wantedBy = [ "mortar-update-auto.service" ];
      after = [ "mortar-update-auto.service" "mortar-packages.socket" ];
      serviceConfig = {
        Type = "oneshot";
        ExecStart = "${exe} auto";
        TimeoutStartSec = "6h";
      };
    };

    # Terminal tools: after everything else, so none of them shadows the system's.
    environment.extraInit = ''
      export PATH="$PATH:${toolsBin}"
    '';
  };
}
