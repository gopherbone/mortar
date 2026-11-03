# SPDX-License-Identifier: GPL-2.0-or-later
# Redemption: Liar (retail GP2X Wiz RPG, pkgs/liar). Same arrangement as PICO-8:
# its own profile, /nix/var/nix/profiles/liar, and the user's own files from their
# card in ~/.local/share/liar, put there by `mortar liar --data PATH`.
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.apps.liar;
in
{
  options.mortar.apps.liar = {
    enable = lib.mkEnableOption "the Redemption: Liar entry, shown once `mortar liar` has installed the loader and your files from the card (see pkgs/liar)" // { default = true; };
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../../pkgs/liar { };
      defaultText = lib.literalExpression "pkgs.callPackage ../../pkgs/liar { }";
      description = ''
        The loader (no game data, no Wiz firmware). Not part of the system:
        `mortar liar` builds this (flake output
        `packages.aarch64-linux.liar`) and installs it into `profile`.
      '';
    };
    profile = lib.mkOption {
      type = lib.types.str;
      default = "/nix/var/nix/profiles/liar";
      description = "The Nix profile the loader is installed into (a GC root of its own, untouched by system updates).";
    };
    bin = lib.mkOption {
      type = lib.types.str;
      readOnly = true;
      default = "${cfg.profile}/bin/liar";
      description = "The launcher inside the profile.";
    };
    data = lib.mkOption {
      type = lib.types.str;
      default = "${config.mortar.user.home}/.local/share/liar/liar_e.gpe";
      defaultText = lib.literalExpression ''"''${config.mortar.user.home}/.local/share/liar/liar_e.gpe"'';
      description = "The file the entry waits for (`requires`); `mortar liar --data` puts it here.";
    };
  };

  config = lib.mkMerge [
    { mortar.unfree = [ "liar" ]; }
    (lib.mkIf cfg.enable {
      mortar.shell.categories.liar = "games";
      mortar.greeter.sessions.liar = {
        name = "Redemption: Liar";
        description = "GP2X Wiz · your own copy";
        icon = "pad";
        order = 28;
        requires = cfg.data;
        # No mortar-sdl-run: a 32-bit process, no GPU, its own Wayland client.
        command = "exec ${cfg.bin}";
      };
    })
  ];
}
