# SPDX-License-Identifier: GPL-2.0-or-later
# Little XM Tracker: an XM tracker for gamepads ("Little XM"); see
# pkgs/little-xm-tracker for controls and data.
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.apps.littlexm;
in
{
  options.mortar.apps.littlexm = {
    enable = lib.mkEnableOption "Little XM Tracker, an XM tracker for gamepads" // { default = true; };
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../../../pkgs/little-xm-tracker { };
      defaultText = lib.literalExpression "pkgs.callPackage ../../../pkgs/little-xm-tracker { }";
      description = "Little XM Tracker (Mortar build) with its launcher.";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    mortar.shell.categories.littlexm = "music";
    mortar.greeter.sessions.littlexm = {
      name = "Little XM";
      description = "Tracker · FastTracker XM";
      icon = "pad";
      order = 23;
      command = "exec ${config.mortar.device.sdlRun}/bin/mortar-sdl-run ${cfg.package}/bin/little-xm-tracker";
    };
  };
}
