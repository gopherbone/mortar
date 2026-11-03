# SPDX-License-Identifier: GPL-2.0-or-later
# What Mortar core, packs and ports need to know about the hardware. A device
# module (devices/brick/default.nix) sets these; everything has a default that
# means "this device doesn't have it", so a new device starts with what it needs.
#
# Assumptions not behind this interface yet are in the Rust sources; README.md
# lists them.
{ lib, pkgs, ... }:

let
  inherit (lib) mkOption types;
in
{
  options.mortar.device = {
    name = mkOption {
      type = types.str;
      default = "Mortar device";
      example = "TrimUI Brick";
      description = "The product name, as the USB gadget reports it to a computer.";
    };

    gpu = {
      libDir = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "GL libraries for the shell's own GPU renderer (null: it draws on the CPU).";
      };
      apps = mkOption {
        type = types.bool;
        default = false;
        description = "Apps started through sdlRun can render on the GPU as Wayland clients (see mortar.shell.gpuApps).";
      };
    };

    sdlRun = mkOption {
      type = types.package;
      default = pkgs.writeShellScriptBin "mortar-sdl-run" ''exec "$@"'';
      defaultText = lib.literalMD "runs the app as it is";
      description = "`bin/mortar-sdl-run APP [ARGS]`: start an SDL app the way this device's GPU needs.";
    };

    suspend = mkOption {
      type = types.bool;
      default = false;
      description = "`systemctl suspend` works and wakes (the default of mortar.shell.ui.suspend).";
    };

    rumble = mkOption {
      type = types.bool;
      default = false;
      description = "It has a rumble motor (the default of mortar.shell.ui.haptics).";
    };

    audio = {
      card = mkOption {
        type = types.str;
        default = "0";
        example = "Codec";
        description = "The built-in sound card, by ALSA id: the default card of the ALSA configuration.";
      };
      wireplumber = mkOption {
        type = types.attrsOf types.anything;
        default = { };
        description = "WirePlumber configuration for the built-in card (with PipeWire on).";
      };
    };

    usb.serialNvmem = mkOption {
      type = types.str;
      default = "";
      description = "Shell glob of nvmem files holding a chip ID, for the USB gadget's serial number (empty: the machine ID).";
    };

    power.usbAdapterCurrent = mkOption {
      type = types.ints.unsigned;
      default = 0;
      description = "Input current limit (uA) on a USB charger (the default of mortar.power.usbAdapterCurrent; 0 leaves it alone).";
    };
  };
}
