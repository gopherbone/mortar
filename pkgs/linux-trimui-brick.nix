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
      {
        # The SPL leaves the A133 NSI DRAM arbiter at reset priorities, so
        # GPU bursts starve the display engine's fetch (garbage band at the
        # top of the panel while the GPU renders). Program the BSP table.
        name = "drm-sun4i-brick-display-engine-dram-priority";
        patch = ../patches/kernel/0003-drm-sun4i-brick-display-engine-dram-priority.patch;
      }
      {
        # The A133 HPOUT gain field is an attenuator (0 = 0 dB, 7 = -42 dB)
        # but was exposed raw, so "Headphone 7/7" was the quietest setting,
        # and the speaker amp is fed from HPOUT. Invert it, add a dB scale.
        name = "asoc-sun4i-codec-a133-headphone-volume-is-attenuation";
        patch = ../patches/kernel/0004-asoc-sun4i-codec-a133-headphone-volume-is-attenuation.patch;
      }
      {
        # Charger and fuel gauge set up like TrimUI's stock driver (4.2 V,
        # 128 mA termination, NTC/JEITA windows, the stock battery model
        # loaded into the gauge), and Charging/Full/Discharging status.
        name = "power-axp2202-stock-charger-setup-gauge-model-charge-state";
        patch = ../patches/kernel/0005-power-axp2202-stock-charger-setup-gauge-model-charge-state.patch;
      }
      {
        # The RTC's "does it tick" check slept 2.5 s in probe, in the
        # middle of the kernel's boot; a worker does it now.
        name = "rtc-trimui-brick-check-the-clock-ticks-off-the-boot-path";
        patch = ../patches/kernel/0006-rtc-trimui-brick-check-the-clock-ticks-off-the-boot-path.patch;
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
      # USB audio class devices on the top (host) port, e.g. the M8 tracker
      SND_USB = yes;
      SND_USB_AUDIO = module;
      # USB MIDI for trackers (ChooChooTracker uses the ALSA sequencer)
      SND_SEQUENCER = module;
      SND_SEQ_MIDI = module;
      USB_HID = module; # USB keyboards and controllers
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
      # --- CPU frequency ---
      # The defconfig boots with the `userspace` governor and nothing ever
      # set a speed, so the A53s sat at the 1008 MHz boot OPP. schedutil
      # follows the scheduler's load (up to the 1992 MHz OPP, 1.25 V via
      # vdd-cpu); `performance` for benchmarks. The cpu-thermal zone's
      # passive trips (58/63 C, step_wise on cpufreq) cap the clock when
      # hot: on 2026-11-01, two cores spinning for 10 minutes at the
      # 1992 MHz setpoint averaged 63 C (peak 71 C) with the clock held at
      # 1992/1800/1608 MHz ~47/49/5% of the time. A PlayStation game ran
      # 1.6x faster per frame at 1992 MHz than at 1008 MHz.
      CPU_FREQ_GOV_SCHEDUTIL = yes;
      CPU_FREQ_GOV_PERFORMANCE = yes;
      CPU_FREQ_DEFAULT_GOV_SCHEDUTIL = yes;
      CPU_FREQ_DEFAULT_GOV_USERSPACE = no;
      # Without utilization clamping, schedutil asks for the top OPP
      # (1992 MHz, 1.25 V) whenever a real-time task is runnable: every
      # PipeWire quantum (5.8 ms) while sound plays, so the clock and
      # vdd-cpu bounced on each one (67 transitions/s with a silent
      # stream, 2026-11-01). With UCLAMP_TASK, real-time tasks get
      # kernel.sched_util_clamp_min_rt_default (set to 0 in
      # devices/brick/hardware.nix) and the clock follows the load.
      UCLAMP_TASK = yes;

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

      # --- zram swap (zramSwap in modules/core/base.nix; without the
      #     module the dev-zram0 job never finishes and blocks reboot) ---
      ZSMALLOC = module;
      ZRAM = module;
      CRYPTO_ZSTD = yes;
      CRYPTO_LZ4 = yes;
      ZRAM_BACKEND_ZSTD = yes;
      ZRAM_BACKEND_LZ4 = yes;

      # --- Suspend to RAM (SCP-backed "deep") ---
      # jkingston's native-deep.fragment (trimui-brick-firmware
      # board/trimui/brick/native-deep.fragment): wake votes through the
      # TF-A/SCP wake service (needs the native-deep DTB's wakeupgen node),
      # and CCU context save/restore for the clocks the SCP powers off.
      # TF-A must be built with SUNXI_A133_SYSTEM_SUSPEND (firmware/mainline/
      # tf-a.nix is); then /sys/power/mem_sleep offers "deep".
      SUSPEND = yes;
      PM_SLEEP = yes;
      SUN50I_A133_WAKEUPGEN = yes;
      SUN50I_A133_CCU_SLEEP = yes;
      # /sys/power/pm_test, pm_print_times, suspend_stats in debugfs
      PM_DEBUG = yes;
      PM_SLEEP_DEBUG = yes;
    };

    extraMeta = {
      branch = "7.3";
      platforms = [ "aarch64-linux" ];
      description = "Linux for the TrimUI Brick (jkingston/trimui-brick-kernel @ ${builtins.substring 0 7 rev})";
    };
  }
  // (args.argsOverride or { })
)
