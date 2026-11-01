# SPDX-License-Identifier: GPL-2.0-or-later
# TrimUI Brick (TG3040) - Allwinner A133 (sun50iw10) handheld
#
# Boot chain:
#   BROM → vendor boot0 (DRAM init) → vendor ATF (patched) →
#   AArch32 shim → RMR → AArch64 trampoline → mainline U-Boot 2026.01 →
#   extlinux.conf → Linux kernel
#
# SD card layout:
#   Sector 16 (8KB):      vendor boot0.bin
#   Sector 32800 (16MB):  modified boot_package (mainline U-Boot + patched ATF)
#   Partition 1 (~128MB): FAT32 /boot (extlinux.conf, kernel, initrd, DTB)
#   Partition 2 (rest):   ext4 NixOS rootfs
#
{ config, lib, pkgs, ... }:

with lib;

let
  cfg = config.hardware.trimui-brick;
  isLiontron = cfg.variant == "liontron";
  isMainline = cfg.variant == "mainline";
in {
  options.hardware.trimui-brick = {
    enable = mkEnableOption "TrimUI Brick (Allwinner A133) hardware support";

    variant = mkOption {
      type = types.enum [ "liontron" "mainline" ];
      default = "liontron";
      description = ''
        Which kernel + device tree to use.

        - `liontron`: nixpkgs linuxPackages_6_18 with the Liontron H-A133L
          DTB and the trimui-brick.dtso overlay (brittonr's original setup).
        - `mainline`: jkingston's trimui-brick-kernel (Linux 7.3) with its
          native sun50i-a133-trimui-brick.dtb.
      '';
    };

    mainlineKernelPackages = mkOption {
      type = types.raw;
      default = pkgs.linuxPackagesFor (pkgs.callPackage ../pkgs/linux-trimui-brick.nix { });
      defaultText = literalExpression
        "pkgs.linuxPackagesFor (pkgs.callPackage ../pkgs/linux-trimui-brick.nix { })";
      description = "Kernel package set used when variant = \"mainline\".";
    };

    extraDtOverlays = mkOption {
      type = types.listOf types.raw;
      default = [ ];
      example = literalExpression ''
        [ { name = "tfa-reserved-memory"; dtsFile = ./tfa-reserved-memory.dtso; } ]
      '';
      description = ''
        (mainline only) Extra hardware.deviceTree.overlays entries for
        sun50i-a133-trimui-brick.dtb, e.g. the bootloader's /reserved-memory
        node for TF-A. Overlays need a root `compatible = "trimui,brick";`
        to be applied. (Appending to hardware.deviceTree.overlays directly
        works too; this is just a discoverable place for it.)
      '';
    };
  };

  config = mkIf cfg.enable (mkMerge [ {

    # --- Boot ---

    boot = {
      # U-Boot loads extlinux.conf from /boot
      loader.grub.enable = false;
      loader.generic-extlinux-compatible.enable = true;

      consoleLogLevel = 7;  # verbose — we need to see everything

      kernelParams = [
        "console=ttyS0,115200n8"
        "earlycon=uart,mmio32,0x05000000"
        "rootwait"
        "nohibernate"
        # Prevent kernel from disabling "unused" regulators/clocks —
        # boot0 configured the hardware, we don't have proper PMIC driver
        "regulator_ignore_unused"
        "clk_ignore_unused"
        # LSM stack (NixOS default)
        "lsm=landlock,yama,bpf"
      ];

      # Minimal initrd modules for SD card boot
      # sunxi-mmc and phy-sun4i-usb are builtins in 6.18
      initrd.availableKernelModules = [
        "ext4"
        "mmc_block"
      ];

      supportedFilesystems = mkForce [ "vfat" "ext4" ];
    };

    # --- Filesystems ---

    fileSystems."/" = {
      device = "/dev/disk/by-label/NIXOS_ROOT";
      fsType = "ext4";
      # commit=120: flush journal every 2 minutes instead of every 5 seconds
      # barrier=0: disable write barriers (we accept crash-risk for SD stability)
      options = [ "noatime" "nodiratime" "commit=120" "barrier=0" ];
    };

    fileSystems."/boot" = {
      device = "/dev/disk/by-label/NIXOS_BOOT";
      fsType = "vfat";
      options = [ "noatime" "umask=0077" ];
    };

    # --- Hardware ---

    hardware = {
      enableRedistributableFirmware = true;
      graphics.enable = false;  # PowerVR GE8300 — no open driver
    };

    # --- Networking ---

    networking = {
      wireless.enable = true;   # RTL8189FTV — needs out-of-tree driver
      useDHCP = true;
      # Reduce DHCP timeout — WiFi driver doesn't work yet, so DHCP on wlan0
      # waits the full timeout (1m47s) generating retry log entries.
      dhcpcd.wait = "background";
    };

    # --- Services ---

    services.openssh = {
      enable = true;
      settings = {
        PermitRootLogin = "yes";
        PasswordAuthentication = true;
      };
    };

    # Serial console — primary interactive console
    systemd.services."serial-getty@ttyS0" = {
      enable = true;
      wantedBy = [ "getty.target" ];
    };

    # --- SD card write reduction ---
    # The SD card fails under sustained writes (~90s into boot: "Card stuck being busy",
    # cascading I/O errors, journal abort, read-only remount). Minimize writes:

    # Journal to RAM only — no persistent journal on SD card
    services.journald.extraConfig = ''
      Storage=volatile
      RuntimeMaxUse=16M
      ForwardToConsole=yes
      TTYPath=/dev/ttyS0
      MaxLevelConsole=info
    '';

    # tmpfs for temp directories
    fileSystems."/tmp" = {
      device = "tmpfs";
      fsType = "tmpfs";
      options = [ "nosuid" "nodev" "size=64M" ];
    };
    fileSystems."/var/tmp" = {
      device = "tmpfs";
      fsType = "tmpfs";
      options = [ "nosuid" "nodev" "size=32M" ];
    };
    fileSystems."/var/log" = {
      device = "tmpfs";
      fsType = "tmpfs";
      options = [ "nosuid" "nodev" "size=32M" ];
    };

    # --- Trim fat ---

    powerManagement.enable = true;
    services.power-profiles-daemon.enable = false;
    documentation.enable = false;
    documentation.nixos.enable = false;
    services.xserver.enable = false;
    security.sudo.wheelNeedsPassword = false;

    # --- Users ---

    users.users.root.initialPassword = "trimui";
  }

  # ===================================================================
  # Liontron variant: nixpkgs 6.18 + Liontron H-A133L DTB + overlay.
  # Everything here is unchanged from brittonr's original module.
  # ===================================================================
  (mkIf isLiontron {
    # Kernel 6.18+ required — has sun50i-a133-liontron-h-a133l.dtb
    boot.kernelPackages = pkgs.linuxPackages_6_18;

    # --- Device Tree ---
    # Apply TrimUI Brick overlay to the Liontron H-A133L DTB:
    #   - Disable PSCI (vendor ATF can't handle AArch64 SMC)
    #   - Disable AXP803 PMIC (TrimUI has AXP2202)
    #   - Add fixed 3.3V regulator for SD card
    #   - Limit SD card speed to 25MHz
    #   - Disable eMMC (needs AXP2202 for voltage)
    #   - Remove pinctrl supply references

    hardware.deviceTree = {
      enable = true;
      filter = "sun50i-a133-liontron-h-a133l.dtb";
      overlays = [
        {
          name = "trimui-brick";
          dtsFile = ../dtb/trimui-brick.dtso;
        }
      ];
    };

    # No framebuffer/display driver yet — disable ALL VT gettys.
    # Without a working console, getty@tty1 autologin crash-loops,
    # generating sustained journal writes that overwhelm the SD card
    # (~90s: "Card stuck being busy" → I/O errors → read-only remount).
    services.logind.settings.Login = {
      NAutoVTs = 0;
      ReserveVT = 0;
    };
    # Mask getty on all VTs — wantedBy override alone is insufficient
    systemd.services."getty@tty1".wantedBy = lib.mkForce [];
    # Also mask the entire getty template to prevent logind from spawning VT gettys
    systemd.targets."getty".wantedBy = lib.mkForce [];
    systemd.services."autovt@".unitConfig.ConditionPathExists = "/dev/null-nonexistent";

  })

  # ===================================================================
  # Mainline variant: jkingston/trimui-brick-kernel + native Brick DTB.
  #
  # Review of the Liontron workarounds against the new kernel/DTS:
  #   - AXP803 PMIC disable, fixed 3.3V vmmc, pinctrl supply redirects,
  #     eMMC disable, EMAC disable: Liontron-DTB-specific. The Brick DTS
  #     describes the real AXP2202 (MFD_TRIMUI_BRICK + regulators) and has
  #     no EMAC, so none of it applies.
  #   - PSCI disable: dropped. The mainline variant targets jkingston's
  #     mainline SPL + TF-A + U-Boot chain, where PSCI/SMP work.
  #     (Booting it through brittonr's vendor-ATF chain would need PSCI
  #     disabled again, as in trimui-brick.dtso.)
  #   - Watchdog disable: dropped. U-Boot arms the hardware watchdog and the
  #     defconfig has WATCHDOG_HANDLE_BOOT_ENABLED + WATCHDOG_OPEN_TIMEOUT=30,
  #     so systemd must take it over (runtimeTime below), or the board
  #     resets ~30 s after the driver probes. watchdog.open_timeout=0 also
  #     covers a slow initrd (e.g. rootwait on a slow card) before systemd
  #     opens /dev/watchdog0. The watchdog is also the restart handler.
  #   - SD 25 MHz / no-1-8-v: not carried over. The Brick DTS runs mmc0 at
  #     SDR104/150 MHz with real vmmc/vqmmc supplies. Root-on-SD write load
  #     is untested on this kernel; if "Card stuck being busy" returns, add
  #     an overlay (extraDtOverlays) capping mmc0 max-frequency at 50 MHz.
  #   - regulator_ignore_unused / clk_ignore_unused: kept (common params).
  #     Harmless and safer during bring-up; drop once the PMIC driver is
  #     known to describe every rail the bootloader enabled.
  #   - journald volatile, tmpfs /tmp,/var/tmp,/var/log, ext4 commit=120:
  #     kept (common). The SD write failures may have been the Liontron mmc
  #     setup, but we have not proven that yet.
  #   - VT getty masking: dropped. The Brick DTS has a working panel/DRM, so
  #     tty1 is a real console now. Journal is in RAM, so a crash-looping
  #     getty would no longer hammer the SD card anyway.
  # ===================================================================
  (mkIf isMainline {
    boot.kernelPackages = cfg.mainlineKernelPackages;

    systemd.watchdog.runtimeTime = mkDefault "15s";
    # VERIFY: newer nixpkgs renames this to
    # systemd.settings.Manager.RuntimeWatchdogSec (old name should still
    # work via mkRenamedOptionModule, with a warning).

    # Serial stays the primary console (/dev/console = last console=), the
    # panel gets kernel messages too. mkBefore puts tty0 ahead of the
    # common console=ttyS0,115200n8.
    boot.kernelParams = mkMerge [
      (mkBefore [ "console=tty0" ])
      [ "watchdog.open_timeout=0" ]
    ];

    # The device kernel only builds what the Brick needs; NixOS' default
    # initrd module list (ahci, xhci_pci, usbhid, ...) mostly doesn't exist.
    boot.initrd.includeDefaultModules = false;
    # Other NixOS modules may still request modules that are built in or
    # absent (e.g. ext2 via EXT4_USE_FOR_EXT2). Tolerate that instead of
    # failing the initrd build.
    nixpkgs.overlays = [
      (final: prev: {
        makeModulesClosure = args: prev.makeModulesClosure (args // { allowMissing = true; });
      })
    ];

    hardware.deviceTree = {
      enable = true;
      filter = "sun50i-a133-trimui-brick.dtb";
      # Also used by image/build-image.nix for the extlinux FDT line, and by
      # generic-extlinux-compatible for on-device rebuilds.
      name = "allwinner/sun50i-a133-trimui-brick.dtb";
      overlays = cfg.extraDtOverlays;
    };

    # The device kernel has nf_tables but no legacy iptables/x_tables.
    networking.nftables.enable = true;

    # Don't rely on getty.target (the Liontron variant forces its wantedBy
    # to []); pull the consoles in from multi-user.target directly.
    systemd.services."serial-getty@ttyS0".wantedBy = [ "multi-user.target" ];
    systemd.services."getty@tty1" = {
      enable = true;
      wantedBy = [ "multi-user.target" ];
    };
  })
  ]);
}
