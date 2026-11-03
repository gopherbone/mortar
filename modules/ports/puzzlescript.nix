# SPDX-License-Identifier: GPL-2.0-or-later
# PuzzleScript engine (C core + Rust/SDL3 shell, pkgs/puzzlescript). Bring your
# own games: it lists ~/.local/share/puzzlescript/games and shows that path while
# the folder is empty.
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.apps.puzzlescript;
in
{
  options.mortar.apps.puzzlescript = {
    enable = lib.mkEnableOption "PuzzleScript" // { default = true; };
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../../pkgs/puzzlescript { };
      defaultText = lib.literalExpression "pkgs.callPackage ../../pkgs/puzzlescript { }";
      description = "The PuzzleScript engine. Games are the user's own.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    mortar.greeter.sessions.games = {
      name = "PuzzleScript";
      description = "Puzzle games, controller";
      icon = "pad";
      order = 10;
      command = "exec ${config.mortar.device.sdlRun}/bin/mortar-sdl-run ${cfg.package}/bin/puzzlescript";
    };
  };
}
