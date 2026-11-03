# SPDX-License-Identifier: GPL-2.0-or-later
# OpenXcom as an app in its own profile, /nix/var/nix/profiles/apps/openxcom: not
# part of the system. `mortar xcom` builds this (flake output
# `packages.aarch64-linux.openxcom`) and installs it. The entry hides until your own
# copy of the game is there; saves and game data stay under
# ~/.local/share/openxcom, outside the profile.
{ pkgs, mkApp }:

mkApp pkgs {
  package = pkgs.callPackage ./. { };
  id = "openxcom";
  name = "OpenXcom";
  description = "UFO Defense · your own copy";
  category = "games";
  icon = "pad";
  gpu = true;
  requires = [ "~/.local/share/openxcom/UFO/GEODATA/WORLD.DAT" ];
}
