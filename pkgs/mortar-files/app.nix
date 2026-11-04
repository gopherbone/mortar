# SPDX-License-Identifier: GPL-2.0-or-later
# Files with a manifest, for its headless test. On a device it's part of the system
# (`mortar.shell.files`, modules/core/shell.nix). Keyboard mode: the pad arrives as
# keys and the shell's on-screen keyboard types names.
{ pkgs, mkApp }:

mkApp pkgs {
  package = pkgs.callPackage ./. { };
  id = "files";
  name = "Files";
  description = "Browse, copy, move, tidy";
  category = "apps";
  icon = "folder";
  gamepad = "keyboard";
}
