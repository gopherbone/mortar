# SPDX-License-Identifier: GPL-2.0-or-later
# The TrimUI Brick as a NixOS module: kernel, boot chain, firmware, GPU, radios
# and the SD image. It works without the Mortar UI: this alone is a console
# system (with root's password "trimui": see hardware.nix; modules/core locks it).
{ config, lib, ... }:

{
  imports = [
    ../../modules/unfree.nix
    ../../modules/device.nix
    ./hardware.nix
    ./firmware.nix
    ./gpu.nix
    ./audio.nix
    ./fastboot.nix
    ./image.nix
    ./rollback.nix
  ];

  hardware.trimui-brick.enable = true;
  nixpkgs.hostPlatform = lib.mkDefault "aarch64-linux";

  # The SCP blob and the U-Boot embedding it, the Wi-Fi/Bluetooth/GPU firmware,
  # the PowerVR userspace. Build locally only; never push these to a public cache.
  mortar.unfree = [ "trimui-brick-scp" "u-boot-trimui-brick-mainline" "trimui-brick-firmware" "pvr-ddk-userspace" ];

  # TF-A /reserved-memory (0x40000000, 0x40000, no-map)
  hardware.trimui-brick.extraDtOverlays = [
    { name = "tf-a-reserved"; dtsFile = ../../firmware/mainline/reserved-memory.dtso; }
  ];
  # FAT /boot is 512 MiB; keep a handful of generations
  boot.loader.generic-extlinux-compatible.configurationLimit = lib.mkDefault 5;

  mortar.fastboot.enable = lib.mkDefault true;

  # What the layers above need to know (modules/device.nix; audio: audio.nix)
  mortar.device = {
    name = "TrimUI Brick";
    gpu.libDir = if config.mortar.gpu.enable then "${config.mortar.gpu.package}/lib" else null;
    gpu.apps = config.mortar.gpu.enable;
    sdlRun = config.mortar.gpu.sdlRun;
    suspend = config.hardware.trimui-brick.suspend.enable;
    rumble = true;
    usb.serialNvmem = "/sys/bus/nvmem/devices/sunxi-sid*/nvmem";
    # Stock: 2.5 A, the driver's maximum.
    power.usbAdapterCurrent = 2500000;
  };
}
