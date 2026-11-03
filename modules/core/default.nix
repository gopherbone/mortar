# SPDX-License-Identifier: GPL-2.0-or-later
# Mortar core: the shell, the greeter, first-boot setup and pairing, battery
# policy, updates, the session user, USB access and audio. Needs a device
# module (devices/brick).
{ lib, pkgs, ... }:

{
  imports = [
    ../unfree.nix
    ../device.nix
    ./usb.nix
    ./usb-watchdog.nix
    ./audio.nix
    ./greeter.nix
    ./shell.nix
    ./setup.nix
    ./power.nix
    ./base.nix
    ./update.nix
    ./user.nix
  ];

  mortar.shell.enable = lib.mkDefault true;
  mortar.greeter.enable = lib.mkDefault true;
  mortar.setup.enable = lib.mkDefault true;
  mortar.power.enable = lib.mkDefault true;

  # After Log out: a login shell on the console (console programs don't run
  # inside the shell, whose own Terminal entry is foot).
  mortar.greeter.sessions.terminal = {
    name = "Terminal";
    description = "Console";
    icon = "term";
    order = 30;
    command = "exec ${pkgs.bashInteractive}/bin/bash --login";
  };

  # Over-the-air updates from a binary cache (docs/updates.md). Nothing
  # here is secret: bucket, endpoint, the public signing key. Set these in your
  # configuration with enable = true and `mortar deploy` once; after that CI (or
  # `mortar publish`) updates the Brick over Wi-Fi. `mortar publish` reads these
  # values too.
  mortar.update = {
    enable = lib.mkDefault false;
    bucket = lib.mkDefault "mortar-cache";
    endpoint = lib.mkDefault "https://ACCOUNT_ID.r2.cloudflarestorage.com";
    publicKey = lib.mkDefault "mortar-1:REPLACE_WITH_PUBLIC_KEY";
  };
}
