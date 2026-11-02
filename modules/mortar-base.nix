# SPDX-License-Identifier: GPL-2.0-or-later
# Mortar base system for the TrimUI Brick (replaces configuration.nix,
# which stays as-is for the original Liontron-based config).
#
# Access model:
#   - No account has a password. Nothing to leak, nothing to guess.
#   - SSH is key-only; root's keys come from <card>/ssh-keys/root
#     (see mortar-usb.nix), the session user's from ssh-keys/<user>.
#   - Sessions on the panel (the greeter, the shell, apps) run as the
#     session user (mortar-user.nix), not root.
#   - Consoles that need physical access (the USB-C serial port, the
#     internal UART pads, text VTs) log in as root automatically.
{ config, lib, pkgs, ... }:

{
  hardware.trimui-brick.enable = true;

  # The first Mortar card was installed with 25.05; keep it.
  system.stateVersion = "25.05";

  networking.hostName = "mortar";
  # No zone in the configuration (it would reveal where the Brick is): the
  # user picks it on the device (mortar-shell Settings > Time zone, i.e.
  # `timedatectl set-timezone`). With null, NixOS leaves /etc/localtime to
  # timedated, so the choice survives deploys and reboots; unset means UTC.
  time.timeZone = lib.mkDefault null;
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

  # Every getty (text VTs, ttyGS0 over USB-C, ttyS0 on the UART pads) logs
  # straight in as root. (The panel's VT1 belongs to the greeter.)
  services.getty.autologinUser = "root";

  environment.systemPackages = with pkgs; [
    htop vim tmux file tree
    evtest
    strace dtc i2c-tools
    usbutils pciutils
    ghostty.terminfo # `ssh` from Ghostty without TERM overrides
    (pkgs.callPackage ../pkgs/mortar-rumble { }) # `mortar attention` buzzes it
    fastfetch
    (pkgs.writeShellScriptBin "screenfetch" ''exec ${pkgs.fastfetch}/bin/fastfetch "$@"'')
  ];

  # fastfetch with a Mortar brick logo (lime/lavender, Pond palette).
  environment.etc."xdg/fastfetch/config.jsonc".text = builtins.toJSON {
    logo = {
      type = "data";
      source = builtins.concatStringsSep "\n" [
        "$1██████████$2████████"
        "$1██████████$2████████"
        "$3              "
        "$2█████$1██████████$2███"
        "$2█████$1██████████$2███"
        "$3              "
        "$1██████████$2████████"
        "$1██████████$2████████"
        "$3              "
        "$3   m o r t a r"
      ];
      color = { "1" = "38;2;206;240;88"; "2" = "38;2;203;166;247"; "3" = "38;2;225;235;247"; };
      padding = { top = 1; left = 1; right = 3; };
    };
    display.color = { keys = "38;2;206;240;88"; title = "38;2;203;166;247"; };
    modules = [
      "title" "separator" "os" "host" "kernel" "uptime" "packages" "shell"
      "display" "wm" "cpu" "gpu" "memory" "swap" "disk" "battery" "sound"
      "break" "colors"
    ];
  };

  # Compressed swap in RAM: 1 GB is tight for heavier tools (Claude Code
  # alone uses ~200 MB at startup).
  zramSwap.enable = true;
  # Swap-in from zram is a decompression, not a disk seek: read one page,
  # not eight (the default page-cluster 3 decompresses 7 pages nobody
  # asked for).
  boot.kernel.sysctl."vm.page-cluster" = 0;

  # No transparent huge pages. With the kernel's default (`always`, and
  # khugepaged filling a 2 MB page from a single used 4 KB one) every
  # process's heap grew in 2 MB steps: 147 MB of the ~400 MB in use at
  # idle were huge pages (the shell 61 MB, WirePlumber 18 MB,
  # mortar-setupd 14 MB of its 20). Measured 2026-11-01 with a fresh foot:
  # 27.5 MB RSS (11.2 MB anonymous) with THP on, 19.8 MB (3.5 MB) without.
  # (`madvise` did not help: foot's heap still got 10 MB of huge pages.)
  boot.kernelParams = [ "transparent_hugepage=never" ];

  services.udisks2.enable = false;
  programs.command-not-found.enable = false;
  systemd.services.systemd-udev-settle.enable = false;
  systemd.network.wait-online.enable = false;

  # Boot time and closure: nothing here reads man pages or the NixOS
  # manual on the device, and journald keeps the logs (no logrotate run
  # at every boot).
  documentation.enable = false;
  documentation.man.enable = false;
  documentation.nixos.enable = false;
  services.logrotate.enable = false;

  # Closure (2026-11-01: 5.5 GB). Speech synthesis is on by default for
  # graphical desktops (greetd counts as one): speech-dispatcher with
  # mbrola voices, flite and Python, ~1 GB and a socket in every session,
  # and nothing on the Brick speaks.
  services.speechd.enable = false;
  # linux-firmware (865 MB) has nothing the Brick loads: XR829 and GPU
  # firmware come from brick-firmware.nix. The Wi-Fi regulatory database
  # stays. For a USB Wi-Fi or Bluetooth dongle that needs firmware:
  # hardware.enableRedistributableFirmware = lib.mkForce true.
  hardware.enableRedistributableFirmware = lib.mkOverride 90 false;
  hardware.wirelessRegulatoryDatabase = true;
}
