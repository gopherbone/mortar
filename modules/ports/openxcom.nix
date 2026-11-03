# SPDX-License-Identifier: GPL-2.0-or-later
# OpenXcom (pkgs/openxcom) with native Brick controls: the engine is in the
# system; the entry hides until `mortar xcom --data PATH` has copied your own UFO:
# Enemy Unknown files.
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.apps.openxcom;
in
{
  options.mortar.apps.openxcom = {
    enable = lib.mkEnableOption "OpenXcom, shown once `mortar xcom --data PATH` has installed your copy of the game" // { default = true; };
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../../pkgs/openxcom { };
      defaultText = lib.literalExpression "pkgs.callPackage ../../pkgs/openxcom { }";
      description = "OpenXcom (Mortar build) with its launcher; no game data.";
    };
    data = lib.mkOption {
      type = lib.types.str;
      default = "${config.mortar.user.home}/.local/share/openxcom/UFO/GEODATA/WORLD.DAT";
      defaultText = lib.literalExpression ''"''${config.mortar.user.home}/.local/share/openxcom/UFO/GEODATA/WORLD.DAT"'';
      description = "A file from the user's copy of the game: the entry waits for it (`requires`).";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    mortar.shell.categories.openxcom = "games";
    mortar.greeter.sessions.openxcom = {
      name = "OpenXcom";
      description = "UFO Defense · your own copy";
      icon = "pad";
      order = 27;
      requires = cfg.data;
      command = "exec ${config.mortar.device.sdlRun}/bin/mortar-sdl-run ${cfg.package}/bin/openxcom";
    };
  };
}
