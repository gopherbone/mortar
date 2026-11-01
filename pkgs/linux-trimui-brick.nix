# SPDX-License-Identifier: GPL-2.0-or-later
#
# Linux kernel for the TrimUI Brick, built from jkingston's mainline port:
#   https://github.com/jkingston/trimui-brick-kernel (branch `brick-hardware`)
#
# That tree is Linux 7.3 plus Brick drivers (panel, AXP2202 PMIC/regulators,
# gpio-keys gamepad, XR829 WiFi/BT, MUSB "trimui,brick-musb" glue, LEDs, ...)
# and ships its own `arch/arm64/configs/trimui_brick_defconfig` together with
# `arch/arm64/boot/dts/allwinner/sun50i-a133-trimui-brick.dts`.
#
# Usage:
#   kernel = pkgs.callPackage ./pkgs/linux-trimui-brick.nix { };
#   boot.kernelPackages = pkgs.linuxPackagesFor kernel;
#
# Why `buildLinux` (with defconfig + enableCommonConfig = false) instead of
# `linuxManualConfig`:
#
#   * buildLinux runs nixpkgs' generate-config.pl on top of
#     `make trimui_brick_defconfig`, so we can *add* options declaratively via
#     `structuredExtraConfig` without carrying a full .config copy that would
#     drift from upstream's defconfig.
#   * With `ignoreConfigErrors = false`, generate-config.pl fails the build if
#     any option below ends up unset or with a different value (e.g. because a
#     Kconfig dependency is missing). That is our guarantee that the NixOS
#     requirements and the USB gadget bits really are in the final .config.
#   * buildLinux kernels carry `passthru.features`, so the NixOS module skips
#     its `system.requiredKernelConfig` eval-time assertions (which would need
#     the .config via import-from-derivation). The options those assertions
#     check are listed explicitly below instead, so the build-time check above
#     covers them.
#   * `enableCommonConfig = false` keeps nixpkgs' huge common-config.nix (which
#     targets generic distro kernels) from overriding the device-tuned
#     defconfig, and `autoModules = false` stops generate-config.pl from
#     answering "m" to every tristate the defconfig left off (aarch64's
#     platform default is autoModules = true).
#
#   linuxManualConfig would give byte-exact control but either needs IFD
#   (allowImportFromDerivation) to satisfy the NixOS assertions, or a
#   hand-maintained `config` attrset that nothing verifies.
#
# VERIFY once Nix is available:
#   - (checked against the pinned nixpkgs 2026-02-04: generic.nix accepts
#     pname, defconfig, enableCommonConfig, autoModules, preferBuiltin,
#     ignoreConfigErrors, and nixos kernel.nix skips requiredKernelConfig
#     assertions for kernels with passthru.features. The flake evaluates.)
#   - modDirVersion matches what the build produces (CONFIG_LOCALVERSION in
#     the defconfig is "-brick-dev"; the build fails loudly if it differs).
#   - generate-config.pl does not complain about any option below.
{
  lib,
  buildLinux,
  fetchFromGitHub,
  ...
}@args:

let
  # The kernel jkingston's trimui-brick-firmware (1161bc7) pins and
  # qualifies: 91f15ef "Bluetooth: hci_xradio: require configured public
  # address" (2026-10-30). `brick-hardware` HEAD (16c9256, 2026-10-31) is 13
  # commits newer and its defconfig rewrite (25238f9) dropped AUTOFS_FS,
  # BPF_SYSCALL, CGROUP_BPF, AUDIT and KEYS; we force those on below anyway,
  # so moving to a newer rev only needs the hash updated.
  rev = "91f15effb15f3752f50d46ac70c8d296c8a9c179";
in
buildLinux (
  args
  // {
    pname = "linux-trimui-brick";
    version = "7.3.0";

    kernelPatches = (args.kernelPatches or [ ]) ++ [
      {
        # The DC (display class) bridge was reconstructed from DDK 1.11;
        # stock 1.19 userspace numbers the commands differently, so window
        # surfaces called the wrong kernel functions. Verified against the
        # stock 4.9 pvrsrvkm.ko and libsrv_um.
        name = "drm-img-dc-bridge-stock-1.19-command-order";
        patch = ../patches/kernel/0001-drm-img-dc-bridge-stock-1.19-command-order.patch;
      }
      {
        # GPU idle power-off can't complete on this power domain and latches
        # the GPU until reboot; keep the GPU powered.
        name = "drm-img-disable-active-power-management";
        patch = ../patches/kernel/0002-drm-img-disable-active-power-management.patch;
      }
    ];
    # Makefile at this rev is 7.3.0; the defconfig sets CONFIG_LOCALVERSION="-brick-dev"
    # and disables LOCALVERSION_AUTO. The fetched source has no .git, so
    # setlocalversion appends nothing else.
    modDirVersion = "7.3.0-brick-dev";

    src = fetchFromGitHub {
      owner = "jkingston";
      repo = "trimui-brick-kernel";
      inherit rev;
      # Placeholder: the first build fails and prints the real hash; paste
      # it here.
      hash = "sha256-oHaXINt0vQ8UI0okdP0UJCRlsPXoVbxe6G1uFLV09kU=";
    };

    defconfig = "trimui_brick_defconfig";
    enableCommonConfig = false;
    autoModules = false;
    preferBuiltin = false;
    ignoreConfigErrors = false;
    # (Rust support is derived from structuredExtraConfig's RUST, which we
    # don't set, so no Rust toolchain is pulled in.)

    structuredExtraConfig = with lib.kernel; {
      # --- What NixOS/systemd require (mirrors system.requiredKernelConfig
      #     in nixos/modules/system/boot/systemd.nix and friends) ---
      DEVTMPFS = yes;
      CGROUPS = yes;
      INOTIFY_USER = yes;
      SIGNALFD = yes;
      TIMERFD = yes;
      EPOLL = yes;
      NET = yes;
      SYSFS = yes;
      PROC_FS = yes;
      FHANDLE = yes;
      CRYPTO_USER_API_HASH = yes;
      CRYPTO_HMAC = yes;
      CRYPTO_SHA256 = yes;

      # iwd (Wi-Fi) refuses to start without these; list from iwd's README
      # "kernel dependencies" plus what it reported missing on the Brick.
      CRYPTO_USER_API_SKCIPHER = yes;
      KEY_DH_OPERATIONS = yes;
      CRYPTO_ECB = yes;
      CRYPTO_CBC = yes;
      CRYPTO_DES = yes;
      CRYPTO_AES = yes;
      CRYPTO_CMAC = yes;
      CRYPTO_MD4 = yes;
      CRYPTO_MD5 = yes;
      CRYPTO_SHA1 = yes;
      CRYPTO_SHA512 = yes;
      ASYMMETRIC_KEY_TYPE = yes;
      ASYMMETRIC_PUBLIC_KEY_SUBTYPE = yes;
      X509_CERTIFICATE_PARSER = yes;
      PKCS7_MESSAGE_PARSER = yes;
      PKCS8_PRIVATE_KEY_PARSER = yes;
      AUTOFS_FS = yes;
      TMPFS = yes;
      TMPFS_POSIX_ACL = yes;
      TMPFS_XATTR = yes;
      SECCOMP = yes;
      AUDIT = yes;
      KEYS = yes;
      # DMIID is also in systemd's requiredKernelConfig list, but on arm64
      # DMI depends on EFI, which this defconfig (and the boot chain) does
      # not use. It is deliberately left off: systemd only uses it for
      # hardware identification (hostnamectl vendor/model), and the NixOS
      # assertion is skipped for buildLinux kernels (they have
      # passthru.features). VERIFY: if `nixos-rebuild`/eval ever reports a
      # DMIID assertion on the pinned nixpkgs, either enable EFI+DMI+DMIID
      # here or filter it out of system.requiredKernelConfig.

      # Other things a NixOS userspace expects
      UNIX = yes;
      IPV6 = yes;
      BLK_DEV_INITRD = yes;
      RD_ZSTD = yes; # boot.initrd.compressor defaults to zstd on >= 5.9
      BPF_SYSCALL = yes; # systemd IPAddressDeny=/RestrictNetworkInterfaces=
      CGROUP_BPF = yes;
      # hardware.firmwareCompression defaults to zstd on >= 5.19, so
      # /lib/firmware/*.zst must be loadable (XR829 firmware etc.)
      FW_LOADER_COMPRESS = yes;
      FW_LOADER_COMPRESS_ZSTD = yes;
      # console=tty0 on the panel
      VT = yes;
      VT_CONSOLE = yes;
      FRAMEBUFFER_CONSOLE = yes;

      # --- nftables, for networking.firewall with networking.nftables ---
      NETFILTER = yes;
      NETFILTER_ADVANCED = yes;
      NF_CONNTRACK = module;
      NF_TABLES = module;
      NF_TABLES_INET = yes;
      NF_TABLES_IPV4 = yes;
      NF_TABLES_IPV6 = yes;
      NFT_CT = module;
      NFT_LOG = module;
      NFT_LIMIT = module;
      NFT_REJECT = module;
      NFT_FIB_IPV4 = module;
      NFT_FIB_IPV6 = module;
      NFT_FIB_INET = module; # used by the firewall's reverse-path filter

      # --- USB gadget over the USB-C port ---
      # The DTS marks usb_otg as compatible = "trimui,brick-musb",
      # "allwinner,sun8i-a33-musb"; drivers/usb/musb/sunxi.c in this tree
      # matches "trimui,brick-musb" directly (5-endpoint config). dr_mode is
      # "peripheral", so MUSB in gadget-only mode (as the defconfig chose) is
      # enough.
      USB_MUSB_HDRC = yes;
      USB_MUSB_SUNXI = yes;
      PHY_SUN4I_USB = yes;
      USB_GADGET = yes;
      USB_LIBCOMPOSITE = yes; # selected by USB_CONFIGFS
      CONFIGFS_FS = yes;
      USB_CONFIGFS = yes;
      USB_CONFIGFS_ACM = yes;
      USB_CONFIGFS_NCM = yes;
      USB_CONFIGFS_ECM = yes;
      USB_CONFIGFS_RNDIS = yes;
      # Lets `console=ttyGS0` be used as a kernel console later; only active
      # when it is on the command line.
      U_SERIAL_CONSOLE = yes;
    };

    extraMeta = {
      branch = "7.3";
      platforms = [ "aarch64-linux" ];
      description = "Linux for the TrimUI Brick (jkingston/trimui-brick-kernel @ ${builtins.substring 0 7 rev})";
    };
  }
  // (args.argsOverride or { })
)
