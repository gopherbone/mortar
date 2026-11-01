# SPDX-License-Identifier: GPL-2.0-or-later
#
# Mainline U-Boot (SPL + proper) for the TrimUI Brick (Allwinner A133), at the
# revision jkingston/trimui-brick-firmware (1161bc7) pins, with his 8 patches
# plus one Mortar patch (1001, SCP-safe load addresses). See README.md.
#
# Output: $out/u-boot-sunxi-with-spl.bin = eGON SPL (LPDDR3 DRAM init, AXP717
# PMIC rails, SCP start) followed by a FIT with TF-A BL31, the vendor SCP +
# its service DTB, and U-Boot proper. Write it at 128 KiB (sector 256) of
# the SD card; partitions must start after it (the sd image uses 8 MiB).
#
# Differences from jkingston's boot flow (his bootcmd loads brick-diag/
# Image + initramfs + DTB and booti's them, with bootcount rollback):
#   * bootcmd = rumble pulse, then `sysboot` /extlinux/extlinux.conf from
#     mmc 0:1 (NixOS generic-extlinux-compatible), so generations work.
#   * load addresses come from the default environment (patch 1001).
#   * bootcount/altbootcmd dropped, MMC/FAT writes off again: U-Boot never
#     writes the card.
#   * CONFIG_WATCHDOG_AUTOSTART kept: the SoC watchdog stays armed into Linux.
#   * BOOTDELAY 0 -> 1 (UART only; there is no video in U-Boot).
#
# Build natively for aarch64-linux (callPackage from aarch64 pkgs). Needs the
# unfree SCP derivation allowed (see scp.nix).
{
  lib,
  buildUBoot,
  fetchFromGitHub,
  callPackage,
  buildPackages,
  tfa ? callPackage ./tf-a.nix { },
  scp ? callPackage ./scp.nix { },
}:

let
  bl31 = "${tfa}/bl31.bin";

  # Kept from jkingston's patch 0006: 120 ms vibration pulse on PH3 as
  # immediate power-on feedback.
  rumble = "gpio set PH3; sleep 0.12; gpio clear PH3";

  # /boot is the FAT p1 (NIXOS_BOOT). The p2 /boot/extlinux fallback only
  # matters if /boot ever lives on the ext4 root instead.
  bootcmd = lib.concatStringsSep "; " [
    rumble
    "sysboot mmc 0:1 any \${scriptaddr} /extlinux/extlinux.conf"
    "sysboot mmc 0:2 any \${scriptaddr} /boot/extlinux/extlinux.conf"
    "echo Mortar: no bootable extlinux.conf on mmc 0:1 or mmc 0:2"
  ];
in
buildUBoot {
  pname = "u-boot-trimui-brick-mainline";
  version = "2026.10-unstable-2026-10-10-5e3b01c8";
  src = fetchFromGitHub {
    owner = "u-boot";
    repo = "u-boot";
    rev = "5e3b01c8466dbe45cd6a06299f8f4c7e8c1c2363";
    hash = "sha256-1ooZvntj2D7NQAyIQa0hdttEKT2lF7LYZlZEFfZ+E34=";
  };

  defconfig = "trimui-brick_defconfig";

  extraPatches = lib.sort (a: b: baseNameOf a < baseNameOf b) (
    map (n: ./patches/u-boot + "/${n}") (builtins.attrNames (builtins.readDir ./patches/u-boot))
  );

  # Appended to .config after `make trimui-brick_defconfig`; later lines win.
  extraConfig = ''
    # Vendor SCP startup in SPL + memreserve of BL31/SCP/vendor regions in
    # ft_board_setup (jkingston enables this from external.mk).
    CONFIG_OF_BOARD_SETUP=y
    CONFIG_SUNXI_A133_VENDOR_SCP=y

    # Standard extlinux boot (NixOS generic-extlinux-compatible).
    CONFIG_CMD_SYSBOOT=y
    CONFIG_CMD_FAT=y
    CONFIG_FS_FAT=y
    CONFIG_CMD_EXT4=y
    CONFIG_FS_EXT4=y
    # CONFIG_EXT4_WRITE is not set
    # CONFIG_CMD_EXT4_WRITE is not set
    CONFIG_CMD_FS_GENERIC=y
    CONFIG_CMD_BOOTI=y
    CONFIG_SUPPORT_RAW_INITRD=y
    CONFIG_BOOTDELAY=1
    CONFIG_USE_BOOTCOMMAND=y
    CONFIG_BOOTCOMMAND="${bootcmd}"

    # Drop jkingston's FAT bootcount rollback (its altbootcmd selects his
    # brick-diag/brick-previous directories, which do not exist here), and
    # with it every U-Boot write path to the card.
    # CONFIG_BOOTCOUNT_LIMIT is not set
    # CONFIG_BOOTCOUNT_FS is not set
    # CONFIG_FAT_WRITE is not set
    # CONFIG_MMC_WRITE is not set
    # CONFIG_CMD_SAVEENV is not set
    CONFIG_ENV_IS_NOWHERE=y

    # Kept: the SoC watchdog is armed in U-Boot and left running into Linux.
    CONFIG_WATCHDOG_AUTOSTART=y
  '';

  extraMakeFlags = [
    "BL31=${bl31}"
    "SCP=${scp}/scp.bin"
    "SCP_FDT=${scp}/service.dtb"
  ];

  filesToInstall = [
    "u-boot-sunxi-with-spl.bin"
    "spl/sunxi-spl.bin"
    ".config"
  ];

  # Sanity-check the image the way jkingston's tools/check_bootloader.py
  # does: eGON header and checksum, FIT right after the SPL, TF-A embedded
  # byte-exact at 0x40000000, U-Boot at 0x4a000000, and the config we want.
  postInstall = ''
    mv $out/.config $out/u-boot.config
    cfg=$out/u-boot.config
    for want in CONFIG_SUNXI_A133_VENDOR_SCP=y CONFIG_CMD_SYSBOOT=y \
                CONFIG_WATCHDOG_AUTOSTART=y CONFIG_OF_BOARD_SETUP=y \
                CONFIG_SUNXI_DRAM_A133_LPDDR3=y CONFIG_DRAM_CLK=672; do
      grep -qx "$want" $cfg || { echo "missing $want in .config" >&2; exit 1; }
    done
    for unwanted in CONFIG_BOOTCOUNT_LIMIT CONFIG_FAT_WRITE CONFIG_MMC_WRITE; do
      if grep -q "^$unwanted=y" $cfg; then echo "$unwanted unexpectedly set" >&2; exit 1; fi
    done

    img=$out/u-boot-sunxi-with-spl.bin
    python3 - "$img" ${bl31} "$cfg" <<'EOF'
    import struct, sys, subprocess, tempfile, os
    FDTGET = '${buildPackages.dtc}/bin/fdtget'
    image = open(sys.argv[1], 'rb').read()
    bl31 = open(sys.argv[2], 'rb').read()
    assert image[4:12] == b'eGON.BT0', 'bad SPL magic'
    checksum, size = struct.unpack_from('<II', image, 12)
    words = list(struct.unpack('<%dI' % (size // 4), image[:size]))
    words[3] = 0x5f0a6c39
    assert sum(words) & 0xffffffff == checksum, 'SPL checksum mismatch'
    off = max(32768, size)
    fit = image[off:]
    # SPL finds the FIT relative to its own size (sunxi spl_mmc raw sector
    # logic, as modelled by jkingston's check_bootloader.py): with the image
    # at 128 KiB it must read exactly 128 KiB + off.
    cfg = dict(l.split('=', 1) for l in open(sys.argv[3]).read().splitlines()
               if l.startswith('CONFIG_') and '=' in l)
    sector = int(cfg['CONFIG_SYS_MMCSD_RAW_MODE_U_BOOT_SECTOR'], 0)
    data_off = int(cfg['CONFIG_SYS_MMCSD_RAW_MODE_U_BOOT_DATA_PART_OFFSET'], 0)
    read_sector = max(sector, size // 512) + (128 - 8) * 2 + data_off
    assert read_sector * 512 == 128 * 1024 + off, 'SPL would read the FIT from the wrong sector'
    magic, total = struct.unpack_from('>II', fit)
    assert magic == 0xd00dfeed and total == len(fit), 'FIT not found right after SPL'
    with tempfile.TemporaryDirectory() as d:
        f = os.path.join(d, 'fit.itb'); open(f, 'wb').write(fit)
        get = lambda n, k, t: subprocess.check_output([FDTGET, '-t', t, f, n, k], text=True).strip()
        data = bytes(int(x, 16) for x in get('/images/atf', 'data', 'bx').split())
        assert data == bl31, 'embedded BL31 differs from tf-a output'
        assert get('/images/atf', 'load', 'x') == '40000000', 'BL31 load address'
        assert get('/images/uboot', 'load', 'x') == '4a000000', 'U-Boot load address'
        imgs = subprocess.check_output([FDTGET, '-l', f, '/images'], text=True).split()
        assert 'scp' in imgs and 'scp-service-dtb' in imgs, 'SCP not in FIT: %r' % imgs
    print('u-boot-sunxi-with-spl.bin: %d bytes (SPL %d, FIT %d at +%#x)' % (len(image), size, len(fit), off))
    # Must end before the first partition (sd image: 8 MiB) when written at 128 KiB.
    assert 128 * 1024 + len(image) <= 8 * 1024 * 1024, 'image too large for the 128K..8M gap'
    EOF
  '';

  extraMeta = {
    description = "Mainline U-Boot SPL + TF-A + SCP image for the TrimUI Brick (A133)";
    platforms = [ "aarch64-linux" ];
    # Embeds the non-redistributable SCP blob.
    license = [ lib.licenses.gpl2Plus lib.licenses.unfree ];
    hydraPlatforms = [ ];
  };

  passthru = { inherit tfa scp; };
}
