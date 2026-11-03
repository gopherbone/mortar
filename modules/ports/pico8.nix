# SPDX-License-Identifier: GPL-2.0-or-later
# PICO-8 is commercial, so it is never part of the system closure (which CI
# builds and the update cache holds). It lives in its own Nix profile on the
# device, /nix/var/nix/profiles/pico8, put there from a computer with
# `mortar pico8` (built from your own zip, see pkgs/pico8). Over-the-air updates
# leave that profile alone; the PICO-8 entries hide while it is missing.
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.apps.pico8;
in
{
  options.mortar.apps.pico8 = {
    enable = lib.mkEnableOption "PICO-8 entries, shown once `mortar pico8` has installed it (see pkgs/pico8)" // { default = true; };
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../../pkgs/pico8 { };
      defaultText = lib.literalExpression "pkgs.callPackage ../../pkgs/pico8 { }";
      description = ''
        PICO-8 built from the user-supplied Raspberry Pi zip. Not part of
        the system: `mortar pico8` builds this (flake output
        `packages.aarch64-linux.pico8`) and installs it into `profile`.
      '';
    };
    profile = lib.mkOption {
      type = lib.types.str;
      default = "/nix/var/nix/profiles/pico8";
      description = "The Nix profile PICO-8 is installed into (a GC root of its own, untouched by system updates).";
    };
    bin = lib.mkOption {
      type = lib.types.str;
      readOnly = true;
      default = "${cfg.profile}/bin/pico8";
      description = "PICO-8 inside the profile (for launcher entries; use it with `requires`).";
    };
  };

  config = lib.mkMerge [
    { mortar.unfree = [ "pico-8" "pico" ]; }
    (lib.mkIf cfg.enable {
      # `pico8` on PATH once installed (the profile is not in the closure)
      environment.profiles = lib.mkAfter [ cfg.profile ];
      mortar.greeter.sessions.pico8 = {
        name = "PICO-8";
        description = "Fantasy console, Splore";
        icon = "pad";
        order = 25;
        requires = cfg.bin;
        command = "exec ${config.mortar.gpu.sdlRun}/bin/mortar-sdl-run ${cfg.bin} -splore";
      };
    })
  ];
}
