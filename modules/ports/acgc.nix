# SPDX-License-Identifier: GPL-2.0-or-later
# Animal Crossing (GameCube decomp port, pkgs/acgc). Same arrangement as PICO-8:
# its own profile, /nix/var/nix/profiles/acgc, and the user's own disc image in
# ~/.local/share/acgc/rom/GAFE01.iso, both put there by `mortar acgc --iso PATH`.
# The entry hides until both exist.
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.apps.acgc;
in
{
  options.mortar.apps.acgc = {
    enable = lib.mkEnableOption "the Animal Crossing entry, shown once `mortar acgc` has installed the port and your disc image (see pkgs/acgc)" // { default = true; };
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../../pkgs/acgc { };
      defaultText = lib.literalExpression "pkgs.callPackage ../../pkgs/acgc { }";
      description = ''
        The Animal Crossing port (no game data). Not part of the system:
        `mortar acgc` builds this (flake output
        `packages.aarch64-linux.acgc`) and installs it into `profile`.
      '';
    };
    profile = lib.mkOption {
      type = lib.types.str;
      default = "/nix/var/nix/profiles/acgc";
      description = "The Nix profile the port is installed into (a GC root of its own, untouched by system updates).";
    };
    bin = lib.mkOption {
      type = lib.types.str;
      readOnly = true;
      default = "${cfg.profile}/bin/acgc";
      description = "The launcher inside the profile.";
    };
    rom = lib.mkOption {
      type = lib.types.str;
      default = "${config.mortar.user.home}/.local/share/acgc/rom/GAFE01.iso";
      defaultText = lib.literalExpression ''"''${config.mortar.user.home}/.local/share/acgc/rom/GAFE01.iso"'';
      description = "The disc image the entry waits for (`requires`); `mortar acgc --iso` puts it here.";
    };
  };

  config = lib.mkMerge [
    { mortar.unfree = [ "acgc" ]; }
    (lib.mkIf cfg.enable {
      environment.profiles = lib.mkAfter [ cfg.profile ];
      mortar.shell.categories.acgc = "games";
      mortar.greeter.sessions.acgc = {
        name = "Animal Crossing";
        description = "GameCube · your own disc";
        icon = "pad";
        order = 26;
        requires = cfg.rom;
        command = "exec ${config.mortar.device.sdlRun}/bin/mortar-sdl-run ${cfg.bin}";
      };
    })
  ];
}
