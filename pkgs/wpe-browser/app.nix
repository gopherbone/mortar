# SPDX-License-Identifier: GPL-2.0-or-later
# WPE Browser as an app in its own profile, /nix/var/nix/profiles/apps/wpe-browser:
# not part of the system. Flake output `packages.aarch64-linux.wpe-browser`.
#
# Built for the shell's `forward` pad (the browser opens only its own clone) and through
# mortar-sdl-run for the GPU. Your data stays outside the profile, in ~/.config,
# ~/.local/share and ~/.cache, so removing the app leaves it alone.
{ pkgs, mkApp, webkit ? null }:

mkApp pkgs {
  package = pkgs.callPackage ./. { inherit webkit; };
  id = "wpe-browser";
  name = "WPE Browser";
  description = "Web browser on WPE WebKit";
  category = "apps";
  icon = "link";
  gpu = true;
  gamepad = "forward";
}
