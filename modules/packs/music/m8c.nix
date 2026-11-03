# SPDX-License-Identifier: GPL-2.0-or-later
# m8c: Dirtywave M8 / M8 Headless client. Plug the M8 into the Brick's TOP USB-C
# port (USB host).
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.apps.m8c;
in
{
  options.mortar.apps.m8c = {
    enable = lib.mkEnableOption "m8c, the M8 tracker client" // { default = true; };
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../../../pkgs/m8c { };
      defaultText = lib.literalExpression "pkgs.callPackage ../../../pkgs/m8c { }";
      description = "m8c with the Brick button config (m8c-brick).";
    };
  };

  config = lib.mkIf cfg.enable {
    environment.systemPackages = [ cfg.package ];
    mortar.greeter.sessions.m8 = {
      name = "M8";
      description = "Tracker on the top USB-C port";
      icon = "usb";
      order = 20;
      command = "exec ${config.mortar.device.sdlRun}/bin/mortar-sdl-run ${cfg.package}/bin/m8c-brick";
    };
  };
}
