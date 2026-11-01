# Mortar

NixOS for the **TrimUI Brick** (TG3040, Allwinner A133), on a mainline
Linux kernel and a fully mainline boot chain. It boots from a microSD card
and leaves the stock OS on the internal eMMC alone: pull the card and the
Brick boots stock again.

It is a small multitasking handheld computer: a controller-driven shell with
GPU-rendered apps, music tools, emulators, PICO-8 and Claude Code, all
configured from this flake. Plug the Brick into a computer with USB-C and
`ssh root@mortar.local` works; updates are built on your computer and pushed
over SSH.

> Status: early but daily-usable. See [What works](#what-works).

## What works

| Area | State |
|---|---|
| Boot | U-Boot SPL → TF-A → U-Boot → `extlinux.conf` → Linux 7.3 → NixOS. All 4 cores. |
| Shell | `mortar-shell`, a Wayland compositor (Rust, Smithay). Apps run fullscreen side by side; **MENU** opens a drawer with live app cards, a launcher and settings. Its UI is drawn on the GPU with spring animations, synthesized UI sounds and haptic ticks. |
| GPU | PowerVR GE8300 on TrimUI's stock DDK 1.19 userspace. GPU apps inside the shell render into dma-bufs (`mortar-wlegl`) that the shell scans out directly; outside the shell, `brick_dc_drm` flips them at 60 fps. A busy M8 screen costs ~12% CPU. |
| Greeter | `mortar-ui` greeter (greetd): pick **Mortar** (the shell) or launch an app directly. |
| Apps | M8 (m8c; plug the M8 into the top port), ChooChooTracker, PICO-8 (your own licensed copy), PuzzleScript, Games (emulators), Terminal, Claude Code. |
| Emulators | `mortar-play`, a minimal libretro frontend with 16 curated, preconfigured systems (GB/GBC/GBA, NES, SNES, Genesis, PS1, arcade, …). Auto-saves and resume, save states, fast-forward. |
| Claude Code | Runs in a persistent tmux session, a controller UI for approving tool calls (A/B/X), and Remote Control from your phone. Sign in with a QR code. |
| Audio | Codec speaker and headphones (ALSA dmix, shared by all apps); USB audio and USB MIDI on the top port. |
| USB-C, bottom port | USB device: serial console + Ethernet over USB. The Brick is `10.55.0.1` / `mortar.local`. A watchdog recovers the link if it goes deaf. |
| USB-C, top port | USB host: the M8, USB audio, keyboards and controllers. |
| SSH | Keys only, from `ssh-keys/root` on the card's FAT boot partition, or added with `mortar pair`. No account has a password. |
| Setup | First-boot setup: a `mortar.toml` file on the card, setup screens on the panel (Wi-Fi list, on-screen keyboard, WPS), and `mortar pair` from your computer (docs/setup-file.md, docs/pairing.md). |
| Wi-Fi, Bluetooth | XR829: 2.4 GHz station mode via iwd; BlueZ. |
| Display | JD9168 DSI panel, 1024×768. |
| Updates | `nix run .#mortar -- deploy`: build, copy, switch. Every generation stays in the boot menu. |
| Debug | Each boot saves logs to `mortar-logs/` on the card's FAT partition. |

The power key sleeps (screen off; any button wakes); hold it for Sleep / Restart / Power off. Set the time zone in Settings.

Not yet: suspend to RAM, charging configuration, LED effects.

## Using it

On the greeter, choose **Mortar**. In the shell:

| Button | Does |
|---|---|
| D-pad, A, B | Move, open, back |
| MENU | Drawer: Apps (live cards; X closes an app), Launch, Settings |
| MENU, then A | Flip back to the previous app |
| VOL+ / VOL− | Volume (the shell owns these keys) |
| VOL+ and VOL− together | Screenshot, saved to `~/Pictures/Screenshots` |
| SELECT + MENU (or F1) | In Games: the game menu (save/load state, reset, quit) |
| START | In Terminal: on-screen keyboard |

Copy games over SSH, e.g. `scp game.gba brick:/var/lib/mortar-play/roms/gba/`
(PS1 needs a BIOS such as `scph5501.bin` in `/var/lib/mortar-play/bios/`;
CHD is the best format for discs). Run `mortar attention` on your computer
to make the Brick buzz.

## Getting started

You need [Nix](https://nixos.org/download) with flakes and a way to build
`aarch64-linux` (a native machine, or on macOS
`nixpkgs#darwin.linux-builder`).

```sh
nix build .#sd-image-mainline          # -> result/sd-image/*.img.zst
zstd -d result/sd-image/*.img.zst -o mortar.img
# write mortar.img to a microSD card (macOS helper: scripts/flash-mac.sh)
```

Insert the card, connect the bottom USB-C port to your computer and power on.
The setup screens show a pairing code; on your computer run:

```sh
nix run .#mortar -- pair      # adds your SSH key and a `Host brick` entry
ssh brick
```

(Or put your public key on the card's `NIXOS_BOOT` partition as
`ssh-keys/root` before the first boot.) After that, change the configuration
in this repo and deploy:

```sh
nix run .#mortar -- deploy            # switch now
nix run .#mortar -- deploy --reboot   # reboot into it and wait (needed for shell/greeter changes)
nix run .#mortar -- rollback
nix run .#mortar -- generations
```

PICO-8 is commercial: add your own Raspberry Pi zip to the Nix store once with
`nix-store --add-fixed sha256 pico-8_0.2.7_raspi.zip` (see pkgs/pico8).

The image and closure contain **non-redistributable** files: TrimUI blobs
(SCP power-management firmware, XR829 and GPU firmware, GPU userspace,
fetched from TrimUI's public firmware release and checked by hash), PICO-8
if enabled, and some emulator cores with non-commercial licences. Never
publish built images or push them to a public binary cache.

## Layout

```
flake.nix                 nixosConfigurations.brick-mainline, sd-image-mainline, apps.mortar
modules/
  trimui-brick.nix        hardware: kernel, DTB, consoles, workarounds
  mortar-base.nix         accounts (no passwords), consoles, base packages
  mortar-usb.nix          USB gadget (ACM + ECM), DHCP, mDNS, SSH, black-box logs
  mortar-usb-watchdog.nix recover a dead USB link
  brick-firmware.nix      XR829/GPU firmware, iwd, Bluetooth address, stable Wi-Fi MAC
  brick-gpu.nix           pvrsrvkm + brick_dc_drm, GPU userspace, mortar-sdl-run
  mortar-greeter.nix      greetd + the mortar-ui greeter, sessions
  mortar-shell.nix        the multitasking shell, its apps and UI options
  mortar-apps.nix         M8, PICO-8, PuzzleScript; audio defaults
  mortar-play.nix         emulators: curated libretro cores + mortar-play
  mortar-ai.nix           Claude Code: tmux session, controller UI, Remote Control
  mortar-setup.nix        first-boot setup: mortar.toml, pairing, setup screens
pkgs/
  linux-trimui-brick.nix  jkingston's Linux 7.3 Brick tree, pinned, + patches/kernel/
  brick-dc-drm/           display-class driver: GPU frames → DRM atomic flips
  pvr-ddk-userspace.nix   stock EGL/GLES libs, patched for NixOS
  sdl3-pvr/               SDL3 on the PowerVR (vivante backend + Wayland via mortar-wlegl)
  mortar-wlegl/           Wayland EGL platform on the stock EGL (GPU apps in the shell)
  mortar-shell/           multitasking Wayland compositor (Rust, Smithay), see DESIGN.md
  mortar-ui/              UI toolkit, theme and the greeter (Rust)
  mortar-play/            libretro frontend (Rust), see DESIGN.md
  mortar-ai/              controller UI for Claude Code + sign-in helper (Rust), see DESIGN.md
  mortar-setup/           setup file, pairing service, setup screens (Rust)
  mortar-pair/            the pairing protocol, shared with mortar-cli
  mortar-cli/             the `mortar` command: deploy, pair, attention, … (Rust)
  mortar-rumble/          rumble motor helper (Rust)
  m8c/, pico8/, trimui-brick-firmware.nix, mortar-gpu-tests/
  choochootracker/        ChooChooTracker (MIT tracker/groovebox), Mortar build + launcher
firmware/mainline/        U-Boot SPL + TF-A + U-Boot, with jkingston's patches
image/build-image-mainline.nix   SD image (bootloader at 128 KiB, FAT /boot, ext4 /)
docs/                     mortar.md, firmware.md, setup-file.md, pairing.md
```

PuzzleScript comes from a separate flake input (`puzzlescript`). The
original Liontron-DTB configuration (`nixosConfigurations.trimui-brick`,
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
- laamaa (m8c), Lexaloffle (PICO-8), the libretro core authors, and
  Butterfly's Pond website for the visual inspiration. The Claude Spark is a
  trademark of Anthropic, used to label the Claude app.

## License

GPL-2.0-or-later for code in this repository (see `LICENSE`). Patches keep
their upstream licenses. Vendor blobs and other third-party binaries are not
part of the repository.
