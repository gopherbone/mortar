# Mortar

NixOS for the **TrimUI Brick** (TG3040, Allwinner A133), on a mainline
Linux kernel and a fully mainline boot chain. It boots from a microSD card
and leaves the stock OS on the internal eMMC alone: pull the card and the
Brick boots stock again.

Plug the Brick into a computer with USB-C and `ssh root@mortar.local` works.
Updates are built on your computer and pushed over SSH.

> Status: early, but daily-usable for tinkering over SSH. See
> [What works](#what-works).

## What works

| Area | State |
|---|---|
| Boot | U-Boot SPL → TF-A → U-Boot → `extlinux.conf` → Linux 7.3 → NixOS. All 4 cores. |
| USB-C, bottom port | USB device: serial console (`/dev/cu.usbmodem*`) + Ethernet over USB. The Brick is `10.55.0.1` / `mortar.local` and hands the computer an address. A watchdog recovers the link if it goes deaf. |
| USB-C, top port | USB host (EHCI/OHCI). USB serial works; USB audio not yet enabled in the kernel. |
| SSH | Keys only, read from `ssh-keys/root` on the card's FAT boot partition. No account has a password. |
| Updates | `nix run .#mortar -- deploy`: build, copy, switch. Every generation stays in the boot menu. |
| Wi-Fi | XR829, 2.4 GHz, station mode, via iwd. |
| Bluetooth | XR829, via BlueZ. |
| Display | JD9168 DSI panel, 1024×768, KMS console. |
| GPU | PowerVR GE8300 with TrimUI's stock DDK 1.19 userspace. EGL window surfaces present on the panel through our display-class driver (`brick_dc_drm`), vsync-paced at 60 fps. GLES also renders into dma-bufs for KMS apps. |
| Buttons | GPIO keys, power key. |
| Apps | PuzzleScript, m8c (M8 tracker client; plug the M8 into the top port). |
| Debug | Each boot saves logs to `mortar-logs/` on the card's FAT partition. |

Not yet: the controller-driven greeter and setup screens, charging
configuration, suspend, audio beyond the driver, USB audio.

## Getting started

You need [Nix](https://nixos.org/download) with flakes and a way to build
`aarch64-linux` (a native machine, or on macOS
`nixpkgs#darwin.linux-builder`).

```sh
nix build .#sd-image-mainline          # -> result/sd-image/*.img.zst
zstd -d result/sd-image/*.img.zst -o mortar.img
# write mortar.img to a microSD card (macOS helper: scripts/flash-mac.sh)
```

Before the first boot, put your public key on the card's `NIXOS_BOOT`
partition as `ssh-keys/root`. Then insert the card, connect the bottom USB-C
port to your computer, power on, and:

```sh
ssh root@mortar.local
```

After that, change the configuration in this repo and deploy:

```sh
nix run .#mortar -- deploy            # switch now
nix run .#mortar -- deploy --reboot   # reboot into it and wait
nix run .#mortar -- rollback
nix run .#mortar -- generations
```

The image and closure contain **non-redistributable** TrimUI blobs (SCP
power-management firmware, XR829 and GPU firmware, GPU userspace). They are
fetched from TrimUI's public firmware release at build time and checked by
hash. Never publish built images or push them to a public binary cache.

## Layout

```
flake.nix                 nixosConfigurations.brick-mainline, sd-image-mainline, apps.mortar
modules/
  trimui-brick.nix        hardware: kernel, DTB, consoles, workarounds
  mortar-base.nix         accounts (no passwords), consoles, base packages
  mortar-usb.nix          USB gadget (ACM + ECM), DHCP, mDNS, SSH, black-box logs
  mortar-usb-watchdog.nix recover a dead USB link
  brick-firmware.nix      XR829/GPU firmware, iwd, Bluetooth address, stable Wi-Fi MAC
  brick-gpu.nix           pvrsrvkm + brick_dc_drm, GPU userspace, GPU tests
  mortar-apps.nix         built-in apps
pkgs/
  linux-trimui-brick.nix  jkingston's Linux 7.3 Brick tree, pinned, + patches/kernel/
  brick-dc-drm/           display-class driver: GPU frames → DRM atomic flips
  pvr-ddk-userspace.nix   stock EGL/GLES libs, patched for NixOS
  trimui-brick-firmware.nix, m8c/, mortar-cli/ (Rust), mortar-gpu-tests/
firmware/mainline/        U-Boot SPL + TF-A + U-Boot, with jkingston's patches
image/build-image-mainline.nix   SD image (bootloader at 128 KiB, FAT /boot, ext4 /)
docs/                     mortar.md, firmware.md
```

The original Liontron-DTB configuration (`nixosConfigurations.trimui-brick`,
`firmware/` vendor-boot0 chain, `re_tools/`, `harness/`, `FINDINGS.md`) is
brittonr's work and kept for reference; it is not what Mortar boots.

## Credits

- **jkingston**: the Linux 7.3 Brick port
  ([trimui-brick-kernel](https://github.com/jkingston/trimui-brick-kernel))
  and the mainline boot chain
  ([trimui-brick-firmware](https://github.com/jkingston/trimui-brick-firmware)),
  which Mortar builds on.
- **brittonr, adeci, murdoa**: the original NixOS on the Brick
  ([trimui-brick-nixos](https://github.com/brittonr/trimui-brick-nixos)),
  which this repository was forked from.
- Imagination Technologies, Allwinner and TrimUI for the GPU DDK and
  firmware; linux-sunxi and everyone upstreaming A133 support.

## License

GPL-2.0-or-later for code in this repository (see `LICENSE`). Patches keep
their upstream licenses. Vendor blobs are not part of the repository.
