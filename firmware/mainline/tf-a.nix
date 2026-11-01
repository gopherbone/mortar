# SPDX-License-Identifier: GPL-2.0-or-later
#
# Mainline TF-A BL31 for the TrimUI Brick (Allwinner A133, PLAT=sun50i_a133),
# at the revision jkingston/trimui-brick-firmware (1161bc7) pins, with his 12
# patches (vendor SCP transport, wake sources, PSCI/system suspend, watchdog
# restart, secondary-CPU restore). See README.md for provenance.
#
# Build flags mirror his external.mk with BR2_PACKAGE_BRICK_SCP=y and
# BR2_PACKAGE_BRICK_SCP_SYSTEM_SUSPEND=y (both set in configs/brick_defconfig).
#
# BL31 is linked at 0x40000000 (DRAM base) and must stay below 0x40040000; the
# U-Boot SPL loads it there from the FIT and the region is reserved for Linux
# (U-Boot ft_board_setup memreserve + reserved-memory.dtso).
#
# Build natively for aarch64-linux (callPackage from aarch64 pkgs).
{
  lib,
  buildArmTrustedFirmware,
  fetchFromGitHub,
}:

let
  platform = "sun50i_a133";
in
(buildArmTrustedFirmware {
  inherit platform;

  # nixpkgs' builder would apply its rk3399 hdcp.bin removal patch and rm the
  # blob; we only build sun50i_a133, so the patch list is ours alone.
  patches = lib.sort (a: b: baseNameOf a < baseNameOf b) (
    map (n: ./patches/tf-a + "/${n}") (builtins.attrNames (builtins.readDir ./patches/tf-a))
  );
  postPatch = "";

  extraMakeFlags = [
    "bl31"
    # TF-A 2.15's toolchain probing insists on an RK3399 Cortex-M0 toolchain
    # for every platform and fails to resolve it from M0_CROSS_COMPILE alone in
    # the nixpkgs builder; name the tools directly (unused for sun50i_a133).
    "rk3399-m0-cc=arm-none-eabi-gcc"
    "rk3399-m0-ld=arm-none-eabi-gcc"
    "rk3399-m0-oc=arm-none-eabi-objcopy"
    "rk3399-m0-od=arm-none-eabi-objdump"
    "rk3399-m0-ar=arm-none-eabi-gcc-ar"
    "SUNXI_VENDOR_SCP_TRANSPORT=1"
    "SUNXI_A133_STANDBY_SCRATCH=1"
    "SUNXI_A133_VENDOR_SCP_STARTUP=1"
    "SUNXI_A133_NATIVE_WAKE_ABI=1"
    "SUNXI_A133_SUSPEND_ORCHESTRATOR=1"
    "SUNXI_A133_PSCI_SUSPEND_PRECURSOR=1"
    "SUNXI_A133_SYSTEM_SUSPEND=1"
    "SUNXI_A133_NATIVE_CPU_SUSPEND=1"
  ];

  filesToInstall = [
    "build/${platform}/release/bl31.bin"
    "build/${platform}/release/bl31/bl31.elf"
  ];

  extraMeta = {
    description = "Mainline TF-A BL31 for the TrimUI Brick (A133) with jkingston's SCP/suspend patches";
    platforms = [ "aarch64-linux" ];
  };
}).overrideAttrs
  (old: {
    version = "2.15-unstable-2026-10-05";
    src = fetchFromGitHub {
      owner = "ARM-software";
      repo = "arm-trusted-firmware";
      rev = "510355478850fb9bb4b064554d3dc2416b8b4b96";
      hash = "sha256-hqjQIGt8sG/96+CUezyxLqZFdybsLM5WnYfCaeBD07g=";
    };

    # BL31 must fit in the 256 KiB that U-Boot/Linux reserve at 0x40000000.
    postInstall = (old.postInstall or "") + ''
      size=$(stat -c %s $out/bl31.bin)
      echo "bl31.bin: $size bytes"
      if [ "$size" -gt $((0x40000)) ]; then
        echo "bl31.bin ($size bytes) exceeds the 0x40000 reservation at 0x40000000" >&2
        exit 1
      fi
    '';
  })
