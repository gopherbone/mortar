# Mainline boot chain for the TrimUI Brick

Nix packaging of jkingston's fully-mainline Brick boot chain
([jkingston/trimui-brick-firmware](https://github.com/jkingston/trimui-brick-firmware)
at `1161bc7`). It has no vendor boot0, no vendor ATF and no AArch32 shim. The
bootloader boots NixOS generations through extlinux.

```
 BROM (SD first, then eMMC)
  │  reads eGON image at 128 KiB (sector 256)
  ▼
 U-Boot SPL  (u-boot-sunxi-with-spl.bin, first 48 KiB)
  │  LPDDR3 DRAM init @672 MHz (patch 0001), AXP717 rails,
  │  loads the FIT from 128 KiB + 48 KiB:
  │    ├─ scp + scp-service-dtb  → sha256-checked, SCP copied to SRAM
  │    │                           0x00100000, service DTB → 0x48100000,
  │    │                           DRAM params exported, SCP reset released
  │    ├─ atf (TF-A BL31)        → 0x40000000 (≤ 0x40000)
  │    └─ uboot                  → 0x4a000000
  ▼
 TF-A BL31 (EL3, PSCI, SCP mailbox, suspend)  ─── stays resident
  ▼
 U-Boot proper (EL2)
  │  rumble pulse (PH3, 120 ms), 1 s autoboot delay (UART),
  │  sysboot mmc 0:1 /extlinux/extlinux.conf     (FAT NIXOS_BOOT)
  │  [fallback: mmc 0:2 /boot/extlinux/extlinux.conf]
  │  ft_board_setup: memreserve BL31, SCP, 0x48000000+16 MiB
  ▼
 Linux (NixOS generation from extlinux.conf: Image, initrd, DTB + overlays)
```

## Files

| File | What |
|---|---|
| `tf-a.nix` | TF-A BL31, `PLAT=sun50i_a133`, with patches in `patches/tf-a/` |
| `scp.nix` | SCP blob + SCP service DTB pulled from TrimUI's stock firmware zip (unfree) |
| `extract-scp.py` | Extractor that `scp.nix` uses (IMAGEWTY → boot_package → items, every step hash-checked) |
| `u-boot.nix` | SPL + U-Boot + FIT (BL31, SCP) → `u-boot-sunxi-with-spl.bin`, with patches in `patches/u-boot/` |
| `reserved-memory.dtso` | DT overlay: `/reserved-memory` no-map for BL31 at 0x40000000 (size 0x40000) |
| `../../image/build-image-mainline.nix` | NixOS module that builds the SD image (wraps nixpkgs `sd-image.nix`) |

## Pinned revisions

| Component | Source | Revision |
|---|---|---|
| TF-A | github.com/ARM-software/arm-trusted-firmware | `510355478850fb9bb4b064554d3dc2416b8b4b96` (2.15 dev, 2026-10-05) |
| U-Boot | github.com/u-boot/u-boot | `5e3b01c8466dbe45cd6a06299f8f4c7e8c1c2363` (2026.10 dev, 2026-10-10) |
| SCP | github.com/trimui/firmware_brick release `v1.1.1-20251126`, `TG3040_Brick_firmware_v1.1.1.zip` | zip sha256 `488db021…916ef` |
| Patches | github.com/jkingston/trimui-brick-firmware `board/trimui/brick/patches/` | `1161bc7dac1df2a4cd32fea525d6b14129301163` (2026-10-30) |

These pins are the same ones jkingston's `configs/brick_defconfig` uses.

## Patch provenance and licenses

All patches except `u-boot/1001-*` are copied unmodified from jkingston's
repository. That repository is GPL-2.0 (`COPYING`). Each patch keeps its
original author header (Jack Kingston, `998656+jkingston@users.noreply.github.com`).
The license that applies is the one on the code each patch adds:

- **`patches/tf-a/0001`–`0012`** are BSD-3-Clause. The new files carry
  `SPDX-License-Identifier: BSD-3-Clause` and "Copyright (c) 2026, The Trusted
  Firmware Project Contributors", which is TF-A's own license. They cover:
  - vendor SCP transport
  - wake-source transport and wake service
  - suspend orchestrator
  - PSCI warm entry
  - system suspend and deep-suspend advertisement
  - native CPU suspend
  - watchdog restart sequence (0010)
  - stock mailbox owners
  - secondary-CPU restore
- **`patches/u-boot/0001`–`0008`** are GPL-2.0+. That is U-Boot's license, and
  the new files carry `SPDX-License-Identifier: GPL-2.0+`; the DTS is
  `(GPL-2.0+ OR MIT)`. The Brick board, LPDDR3 timings and AXP717 setup in 0001
  are credited to ericreinsmidt/plastron `414735de`. The patches cover:
  - 0001: SD boot, DRAM timings (`dram_timings/a133_lpddr3.c`), PMIC rails, `trimui-brick_defconfig` and the DTS
  - 0002, 0003, 0006: bootcmd-only defconfig edits (payload fallback, bootcount rollback, rumble)
  - 0004: SCP start
  - 0005: FEL isolation from bootcount
  - 0007: msgbox clock
  - 0008: XR829 reference clocks
- **`patches/u-boot/1001-*`** was written for Mortar and is GPL-2.0+. It sets
  load addresses that are safe from the SCP, and the default `fdtfile` (see
  below).
- **`extract-scp.py`** is adapted from jkingston's `package/brick-scp/extract.py`
  and is GPL-2.0-only, like its source. It streams the image instead of loading
  it all into memory.

We keep all 8 of jkingston's U-Boot patches so they apply cleanly. The
bootcmd/bootcount changes in 0002, 0003 and 0006 are then overridden by
`extraConfig` in `u-boot.nix`.

## Blobs

The SCP firmware (`scp.bin`, 0x14008 bytes, sha256 `b25eacb0…bbe8`) and the SCP
service DTB (`service.dtb`, 0x25600 bytes, sha256 `06c820c4…f7d8`) are
proprietary. They are not redistributable.

- `scp.nix` fetches TrimUI's public release zip at build time with a fixed
  hash, as jkingston's Buildroot package does.
- **Never commit either blob, the U-Boot output or an SD image to git.** Never
  push them to a public cache. `scp.nix` and `u-boot.nix` set
  `meta.license = unfree`, `hydraPlatforms = [ ]` and `allowSubstitutes = false`.
- Evaluating them requires allowing unfree for `trimui-brick-scp` and
  `u-boot-trimui-brick-mainline`.

The SPL re-checks both SHA-256 values at boot (patch 0004) and panics on a
mismatch. The user's old Bloom card holds a byte-identical `scp.bin` at
boot_package (sector 32800) + 0xd1c00. Its `dtb` item is a different tree
(`617081ab…`), though, so the card cannot provide `service.dtb`.

## What changed vs jkingston's boot flow

| | jkingston | Mortar |
|---|---|---|
| bootcmd | load `brick-{diag,previous,baseline}/Image`, `initramfs.cpio.gz`, `brick-usb.dtb`, then `fdt rsvmem add 0x40000000 0x40000` and `booti` | rumble pulse, then `sysboot mmc 0:1 any ${scriptaddr} /extlinux/extlinux.conf` (then mmc 0:2 `/boot/extlinux/extlinux.conf`) |
| Kernel/initrd/DTB | fixed directory | whatever generic-extlinux-compatible wrote: every NixOS generation, with a menu on UART |
| Root fs | initramfs (`rdinit=/init`) | ext4 `NIXOS_ROOT` (p2); `/boot` = FAT `NIXOS_BOOT` (p1, 512 MiB at 8 MiB) |
| Load addresses | in bootcmd: kernel 0x42000000, fdt 0x44000000, initrd 0x45000000 | default env (patch 1001): kernel 0x40200000, fdt 0x46000000, script 0x46100000, pxefile 0x46200000, overlays 0x46300000, initrd 0x49000000 (624 MiB room), kernel_comp 0x70000000; `fdt_high`/`initrd_high` = ~0 |
| BL31 reservation | `fdt rsvmem` in bootcmd, plus `ft_board_setup` memreserve | `ft_board_setup` memreserve (unchanged, runs for sysboot too), plus `reserved-memory.dtso` (no-map) |
| Bootcount rollback | `BOOTCOUNT_FS` `/brick-bootcount`, limit 1, `altbootcmd` boots `brick-previous`/`brick-baseline` | dropped (see below) |
| Card writes from U-Boot | `MMC_WRITE`, `FAT_WRITE` (for the bootcount) | none (`MMC_WRITE`, `FAT_WRITE`, `EXT4_WRITE`, `SAVEENV` off; `ENV_IS_NOWHERE`) |
| Boot delay | 0 | 1 s (UART only; U-Boot has no video) |
| Watchdog | `WATCHDOG_AUTOSTART` | unchanged |
| `fdtfile` default | `allwinner/allwinner/…dtb` (doubled prefix, unused by his bootcmd) | `allwinner/sun50i-a133-trimui-brick.dtb` (matters only for extlinux `FDTDIR`) |

Why the load addresses changed: the SPL leaves the SCP service DTB at
0x48100000 and reserves 0x48000000–0x48ffffff. The stock sunxi arm64
environment lets bootm relocate the initrd and FDT to just below
`bootm_size` (0x4a000000). That puts any initrd larger than 16 MiB on top of
the SCP region. A NixOS initrd is easily that size. Mortar's layout loads the
initrd above 0x49000000 and boots it in place.

## Watchdog

- U-Boot arms the SoC watchdog (`WATCHDOG_AUTOSTART`, `WDT_SUNXI`, 16 s
  timeout) and services it while it runs. That includes the extlinux menu and
  the U-Boot prompt.
- The watchdog is still running when Linux starts. jkingston's `sunxi_wdt`
  sees it running at probe and keeps it going. With the kernel defaults
  (`WATCHDOG_HANDLE_BOOT_ENABLED`, `CONFIG_WATCHDOG_OPEN_TIMEOUT=30`), the
  kernel pets it until userspace opens `/dev/watchdog`. If nothing opens it
  within 30 s, the board resets.
- A hang before the driver probes, for example in early kernel init, resets
  after 16 s.
- On NixOS, systemd should own the watchdog:
  `systemd.settings.Manager.RuntimeWatchdogSec = "15s"` (the hardware maximum
  is 16 s). Add either `watchdog.open_timeout=0` (wait forever for userspace)
  or a generous value, so that a slow initrd or first-boot root resize cannot
  trip it. `modules/trimui-brick.nix` (mainline variant) already sets both.
- jkingston's own userspace stops petting after 60 s if boot has not finished,
  so that the reset triggers his rollback. Mortar has no rollback, so
  deliberately starving the watchdog would only cause a reboot loop.

## Bootcount / fallback

Dropped. jkingston's scheme only works with his fixed directories:

- U-Boot increments `/brick-bootcount` on FAT.
- Above `BOOTLIMIT=1`, the `altbootcmd` boots `brick-previous/`.
- Userspace resets the counter.

extlinux has no label that means "previous generation". NixOS labels are
`nixos-default` and `nixos-<N>-default`, and the generation numbers change.

To bring rollback back on top of extlinux:

1. Re-enable `BOOTCOUNT_LIMIT`, `BOOTCOUNT_FS`, `FAT_WRITE` and `MMC_WRITE` in
   `u-boot.nix`.
2. Set `altbootcmd` to `sysboot mmc 0:1 any ${scriptaddr} /extlinux/fallback.conf`.
3. Add a NixOS boot-success unit that:
   - writes `fallback.conf` (a copy of `extlinux.conf` whose `DEFAULT` is the
     last generation that booted successfully)
   - zeroes `/boot/brick-bootcount`
4. Optionally, starve the watchdog when boot does not finish.

`setenv pxe_label_override <label>` in `altbootcmd` is an alternative to step
2, but userspace would still have to record the label. Without rollback,
choose an older generation from the extlinux menu on UART. On-device
selection without UART would need a `gpio input` key check in bootcmd.

## Using it

The integration lives in `flake.nix` and `modules/`.

- Packages are `pkgs.callPackage ./firmware/mainline/u-boot.nix { }` and the
  matching `tf-a.nix` and `scp.nix`. Build them natively for aarch64-linux.
- For the SD image, add `(import ./image/build-image-mainline.nix { })` to the
  NixOS modules. Then build `config.system.build.sdImage`.
- Apply the overlay with
  `hardware.deviceTree.overlays = [ { name = "tf-a-reserved"; dtsFile = ./firmware/mainline/reserved-memory.dtso; } ]`.
