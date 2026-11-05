# SPDX-License-Identifier: GPL-2.0-or-later
# Packages with a manifest, for its headless test. On a device it's part of the system
# (`mortar.shell.packages`, modules/core/shell.nix). Keyboard mode: the pad arrives as
# keys and the shell's on-screen keyboard types searches.
{ pkgs, mkApp }:

mkApp pkgs {
  package = pkgs.callPackage ./. { };
  id = "packages";
  name = "Packages";
  description = "Get more for your Brick";
  category = "apps";
  icon = "box";
  gamepad = "keyboard";
}
