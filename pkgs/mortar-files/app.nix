# SPDX-License-Identifier: GPL-2.0-or-later
# Files as an app in its own profile, /nix/var/nix/profiles/apps/files: not part
# of the system. `mortar files` builds this (flake output
# `packages.aarch64-linux.mortar-files`) and installs it. Keyboard mode: the pad
# arrives as keys and the shell's on-screen keyboard types names.
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
