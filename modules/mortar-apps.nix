# SPDX-License-Identifier: GPL-2.0-or-later
# Built-in apps. They become launcher entries once the greeter/launcher
# lands; for now they are just installed.
#
#   puzzlescript  PuzzleScript engine (C core + Rust/SDL3 shell)
#   m8c           Dirtywave M8 / M8 Headless client. Plug the M8 into the
#                 Brick's TOP USB-C port (USB host). Needs OpenGL (GPU work)
#                 and USB audio in the kernel for sound.
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.apps;
in
{
  options.mortar.apps = {
    puzzlescript = {
      enable = lib.mkEnableOption "PuzzleScript" // { default = cfg.puzzlescript.package != null; };
      package = lib.mkOption {
        type = lib.types.nullOr lib.types.package;
        default = null;
        description = "PuzzleScript package (from the puzzlescript flake input).";
      };
    };
    m8c = {
      enable = lib.mkEnableOption "m8c, the M8 tracker client" // { default = true; };
      package = lib.mkOption {
        type = lib.types.package;
        default = pkgs.callPackage ../pkgs/m8c { };
        defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/m8c { }";
        description = "m8c with the Brick button config (m8c-brick).";
      };
    };
  };

  config = {
    environment.systemPackages =
      lib.optional (cfg.puzzlescript.enable && cfg.puzzlescript.package != null) cfg.puzzlescript.package
      ++ lib.optional cfg.m8c.enable cfg.m8c.package;
  };
}
