# SPDX-License-Identifier: GPL-2.0-or-later
# The full configuration: every pack and port this repository may publish.
# Claude Code (`nixosModules.pack-ai`) is not in it: it is opt-in, for your own flake
# (modules/unfree.nix).
# OpenXcom is not among them either: it is an app in its own profile,
# pkgs/openxcom/app.nix.
#
# The leaf modules are listed one by one, in this order, because module order
# decides the order of merged lists (system packages, kernel parameters, ...;
# later modules come first), and with it the store path of the system. For the
# same system, keep the order.
{ lib, ... }:

{
  imports = [
    ../devices/brick/hardware.nix
    ../modules/core/usb.nix
    ../modules/core/usb-watchdog.nix
    ../devices/brick/firmware.nix
    ../devices/brick/gpu.nix
    ../modules/core/audio.nix
    ../modules/packs/music/littlexm.nix
    ../modules/packs/music/choochoo.nix
    ../modules/packs/music/m8c.nix
    ../modules/ports/puzzlescript.nix
    ../modules/ports/acgc.nix
    ../modules/ports/pico8.nix
    ../modules/core/greeter.nix
    ../modules/core/shell.nix
    ../modules/core/setup.nix
    ../modules/packs/emulation.nix
    ../modules/core/power.nix
    ../modules/core/base.nix
    ../devices/brick/fastboot.nix
    ../modules/core/update.nix
    ../modules/core/user.nix
    ../modules/packs/terminal.nix
    ../devices/brick/image.nix

    ../devices/brick/audio.nix
    ../devices/brick
    ../modules/core
    ../modules/packs/music
  ];

  mortar.update.configName = lib.mkDefault "brick-full";
}
