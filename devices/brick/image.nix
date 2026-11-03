# SPDX-License-Identifier: GPL-2.0-or-later
#
# SD card image for the TrimUI Brick on the fully-mainline boot chain
# (mainline U-Boot SPL -> TF-A BL31 + vendor SCP -> U-Boot -> sysboot
# extlinux.conf -> Linux). See firmware/mainline/README.md.
#
# This is a NixOS module, not a hand-rolled builder: it reuses nixpkgs'
# installer/sd-card/sd-image.nix (sized-to-fit image, deterministic FAT,
# first-boot root resize, nix-store --load-db registration) and only adds
# the Brick specifics. Usage:
#
#   modules = [ ./devices/brick ];                     # imports this, default U-Boot
#   => config.system.build.sdImage  (sd-image/<name>.img.zst)
#
# Layout (MBR, 512-byte sectors):
#
#   sector 0          MBR
#   sector 256        u-boot-sunxi-with-spl.bin (128 KiB; BROM loads it from
#                     here). ~873 KiB today, so it ends near 1 MiB; the build
#                     fails if it would reach the first partition.
#   8 MiB             p1 FAT, label NIXOS_BOOT, 512 MiB, mounted at /boot:
#                       extlinux/extlinux.conf, nixos/*-Image, *-initrd,
#                       *-dtbs/ (written by generic-extlinux-compatible, so
#                       every nixos-rebuild adds a generation here)
#   8 MiB + 512 MiB   p2 ext4, label NIXOS_ROOT, the system closure; grown to
#                     fill the card on first boot (sdImage.expandOnBoot)
#
# U-Boot runs `sysboot mmc 0:1 any ${scriptaddr} /extlinux/extlinux.conf`.
#
# The SPL/FIT embeds the non-redistributable SCP blob, so the image is too:
# do not publish it. The configuration must allow the unfree SCP derivation
# (see firmware/mainline/scp.nix).
{ config, lib, pkgs, modulesPath, ... }:

let
  uboot = pkgs.callPackage ../../firmware/mainline/u-boot.nix { };

  bootloader = "${uboot}/u-boot-sunxi-with-spl.bin";

  # BROM reads the eGON image from 128 KiB (sector 256).
  bootloaderSector = 256;
  # Partition 1 starts here (MiB). Leaves 8 MiB - 128 KiB for SPL + FIT.
  firstPartitionMiB = 8;
in
{
  imports = [ "${modulesPath}/installer/sd-card/sd-image.nix" ];

  config = {
    # sd-image.nix turns this on for generic installer images; it adds initrd
    # modules the Brick kernel does not build.
    hardware.enableAllHardware = lib.mkForce false;

    boot.loader.grub.enable = lib.mkDefault false;
    boot.loader.generic-extlinux-compatible.enable = lib.mkDefault true;

    # The FAT partition *is* /boot (sd-image.nix only knows a FIRMWARE
    # partition at /boot/firmware). Its noauto /boot/firmware entry now points
    # at the same partition and stays unmounted; harmless.
    fileSystems."/boot" = {
      device = lib.mkDefault "/dev/disk/by-label/NIXOS_BOOT";
      fsType = lib.mkDefault "vfat";
    };

    image.baseName = lib.mkDefault "mortar-trimui-brick-${config.system.nixos.label}";

    sdImage = {
      firmwarePartitionOffset = firstPartitionMiB;
      firmwarePartitionName = "NIXOS_BOOT";
      firmwareSize = 512;
      rootVolumeLabel = "NIXOS_ROOT";

      # Populate the FAT /boot exactly like an on-device switch-to-configuration
      # would (extlinux.conf + kernel/initrd/dtbs of this generation).
      populateFirmwareCommands = ''
        ${config.boot.loader.generic-extlinux-compatible.populateCmd} \
          -c ${config.system.build.toplevel} -d ./firmware
      '';

      # /boot is a mount point on the root fs; nothing else goes there.
      populateRootCommands = ''
        mkdir -p ./files/boot
      '';

      postBuildCommands = ''
        bl=${bootloader}
        blSize=$(stat -c %s "$bl")
        blEnd=$(( ${toString bootloaderSector} * 512 + blSize ))
        p1Start=$(( $(partx "$img" -g -o START --nr 1) * 512 ))
        echo "bootloader: $blSize bytes at ${toString bootloaderSector}*512, ends at $blEnd; p1 at $p1Start"
        if [ "$blEnd" -gt "$p1Start" ]; then
          echo "u-boot-sunxi-with-spl.bin overlaps partition 1" >&2
          exit 1
        fi
        dd if="$bl" of="$img" bs=512 seek=${toString bootloaderSector} conv=notrunc status=none
      '';
    };

    system.build.mortarBootloader = uboot;
  };
}
