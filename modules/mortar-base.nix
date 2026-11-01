# SPDX-License-Identifier: GPL-2.0-or-later
# Mortar base system for the TrimUI Brick (replaces configuration.nix,
# which stays as-is for the original Liontron-based config).
#
# Access model:
#   - No account has a password. Nothing to leak, nothing to guess.
#   - SSH is key-only; root's keys come from <card>/ssh-keys/root
#     (see mortar-usb.nix).
#   - Consoles that need physical access (the screen, the USB-C serial
#     port, the internal UART pads) log in as root automatically.
{ config, lib, pkgs, ... }:

{
  hardware.trimui-brick.enable = true;

  # The first Mortar card was installed with 25.05; keep it.
  system.stateVersion = "25.05";

  networking.hostName = "mortar";
  time.timeZone = lib.mkDefault "UTC";
  i18n.defaultLocale = "en_US.UTF-8";

  nix.settings = {
    experimental-features = [ "nix-command" "flakes" ];
    auto-optimise-store = true;
    max-jobs = 1;
    cores = 2;
  };

  # Users are fully declarative: anything not listed here (like the old
  # `gamer` account) is removed on the next switch.
  users.mutableUsers = false;
  users.users.root = {
    initialPassword = lib.mkForce null; # trimui-brick.nix sets "trimui"
    hashedPassword = "!"; # locked
  };
  # NixOS can't see the keys on the card (<card>/ssh-keys/root) and would
  # otherwise refuse to build a system where root has no password.
  users.allowNoPasswordLogin = true;

  # Every getty (tty1 on the screen, ttyGS0 over USB-C, ttyS0 on the UART
  # pads) logs straight in as root.
  services.getty.autologinUser = "root";

  environment.systemPackages = with pkgs; [
    htop vim tmux file tree
    evtest
    strace dtc i2c-tools
    usbutils pciutils
    (pkgs.callPackage ../pkgs/mortar-rumble { }) # `mortar attention` buzzes it
  ];

  # Compressed swap in RAM: 1 GB is tight for heavier tools (Claude Code
  # alone uses ~200 MB at startup).
  zramSwap.enable = true;

  services.udisks2.enable = false;
  programs.command-not-found.enable = false;
  systemd.services.systemd-udev-settle.enable = false;
  systemd.network.wait-online.enable = false;
}
