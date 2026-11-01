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
| Shell | `mortar-shell`, a Wayland compositor (Rust, Smithay). Apps run fullscreen side by side; **MENU** opens your running apps as live full-screen windows; ▶ brings the cross menu: launchers by category, settings (Wi-Fi, Bluetooth, a per-app mixer) and power, over a quiet animated pattern per category. Its UI is drawn on the GPU with spring animations, synthesized UI sounds and haptic ticks. |
| GPU | PowerVR GE8300 on TrimUI's stock DDK 1.19 userspace. GPU apps inside the shell render into dma-bufs (`mortar-wlegl`) that the shell scans out directly; outside the shell, `brick_dc_drm` flips them at 60 fps. A busy M8 screen costs ~12% CPU. |
| Greeter | `mortar-ui` greeter (greetd): pick **Mortar** (the shell) or launch an app directly. |
| Apps | M8 (m8c; plug the M8 into the top port), ChooChooTracker, PICO-8 (your own licensed copy), PuzzleScript, Games (emulators), Terminal, Claude Code. |
| Emulators | `mortar-play`, a minimal libretro frontend with 16 curated, preconfigured systems (GB/GBC/GBA, NES, SNES, Genesis, PS1, arcade, …). Auto-saves and resume, save states, fast-forward. |
| Claude Code | Runs in a persistent tmux session, a controller UI for approving tool calls (A/B/X), and Remote Control from your phone. Sign in with a QR code. |
| Audio | PipeWire in the session: codec speaker and headphones, USB audio and USB MIDI on the top port, Bluetooth headphones. 5.8 ms quantum for the trackers. |
| USB-C, bottom port | USB device: serial console + Ethernet over USB. The Brick is `10.55.0.1` / `mortar.local`. A watchdog recovers the link if it goes deaf. |
| USB-C, top port | USB host: the M8, USB audio, keyboards and controllers. |
| Accounts | The panel's sessions (shell, apps, Claude Code) run as the user `brick`; root is for SSH (deploys) and the serial consoles. Time zone, power and backlight go through polkit/udev, Wi-Fi and Bluetooth through `mortar-setupd`. |
| SSH | Keys only, from `ssh-keys/root` (and optionally `ssh-keys/brick`) on the card's FAT boot partition, or added with `mortar pair`. No account has a password. |
| Setup | First-boot setup: a `mortar.toml` file on the card, setup screens on the panel (Wi-Fi list, on-screen keyboard, WPS), and `mortar pair` from your computer (docs/setup-file.md, docs/pairing.md). |
| Wi-Fi, Bluetooth | XR829: 2.4 GHz station mode via iwd; BlueZ with pairing in Settings > Bluetooth (headphones over A2DP, controllers forwarded to apps like the built-in pad). |
| Display | JD9168 DSI panel, 1024×768. |
| Updates | Over Wi-Fi: CI builds each push and publishes it to a private binary cache; the Brick downloads, stages and asks you to restart (docs/updates.md). Or `nix run .#mortar -- deploy` over USB. Every generation stays in the boot menu. |
| Battery | Charger set up like stock (4.2 V, 960 mA, 128 mA termination, NTC/JEITA windows); the AXP2202 fuel gauge runs TrimUI's battery model. Charging/full shown in the status bars; warnings at 15 % and 5 %; clean power-off at 3 %. USB input: 500 mA from a computer, 2.5 A from a charger. `mortar-power status`. |
| Debug | Each boot saves logs to `mortar-logs/` on the card's FAT partition. |

Tap the power key to sleep: the screen goes dark and the Brick suspends (s2idle) after 3 s; the power key or any gamepad button wakes it. Settings can also sleep it when idle; hold it for Sleep / Restart / Power off. Set the time zone in MENU > Settings > More settings.

Not yet: deep suspend (it resumes with a fuzzy panel, see modules/trimui-brick.nix), LED effects.

## Using it

On the greeter, choose **Mortar**. In the shell:

| Button | Does |
|---|---|
| D-pad, A, B | Move, open, back |
| MENU | Running: your open windows, Home first, stacked full screen; MENU or B closes it |
| MENU, then A | Flip back to the previous app |
| In Running: ▲▼, A, X, Y | Scroll, switch to it, close it, options (switch, restart, close) |
| In Running: ▶ (R1) | The cross: Apps, Games, Music, Settings (R1: straight to Settings); ◀ from Apps or L1 goes back |
| In the cross: ◀▶, ▲▼, A | Category, item, open |
| Settings | Brightness and volume (A, then ◀▶, A when done), Mixer, Wi-Fi, Bluetooth, Sleep, More settings (sounds, motion, palette, time zone, remote access, log out), Restart and Power off (press A twice) |
| Settings › Wi-Fi | On/off, the network you are on (details, forget), networks nearby (A joins; a password keyboard for new secured ones) |
| Settings › Bluetooth | On/off, your devices (A connects or disconnects, X forgets), Pair new device |
| Mixer (Music or Settings) | A fader per app, Master and UI sounds: ◀▶ channel, ▲▼ level, A mute |
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

PICO-8 is commercial, so it is not part of the system: add your own Raspberry
Pi zip to your computer's Nix store once with
`nix-store --add-fixed sha256 pico-8_0.2.7_raspi.zip` (see pkgs/pico8), then
`nix run .#mortar -- pico8` installs it into its own profile on the Brick,
where over-the-air updates leave it alone.

Over-the-air updates (GitHub Actions + a private Cloudflare R2 bucket, or
`mortar publish` from your computer) are set up once: docs/updates.md.

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
  mortar-base.nix         root (no password), consoles, base packages
  mortar-user.nix         the session user, polkit rules, data moved over from root
  mortar-usb.nix          USB gadget (ACM + ECM), DHCP, mDNS, SSH, black-box logs
  mortar-usb-watchdog.nix recover a dead USB link
  brick-firmware.nix      XR829/GPU firmware, iwd, Bluetooth address, stable Wi-Fi MAC
  brick-gpu.nix           pvrsrvkm + brick_dc_drm, GPU userspace, mortar-sdl-run
  mortar-greeter.nix      greetd + the mortar-ui greeter, sessions
  mortar-shell.nix        the multitasking shell, its apps and UI options
  mortar-apps.nix         M8, PICO-8, PuzzleScript; PipeWire and codec defaults
  mortar-play.nix         emulators: curated libretro cores + mortar-play
  mortar-ai.nix           Claude Code: tmux session, controller UI, Remote Control
  mortar-setup.nix        first-boot setup: mortar.toml, pairing, setup screens
  mortar-power.nix        battery policy: USB input current, low-battery warnings, power-off
  mortar-update.nix       over-the-air updates from a private binary cache
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
  mortar-setup/           setup file, pairing service, Wi-Fi/Bluetooth, setup screens (Rust)
  mortar-pair/            the pairing protocol, shared with mortar-cli
  mortar-cli/             the `mortar` command: deploy, pair, publish, pico8, … (Rust)
  mortar-update/          over-the-air updates on the Brick + the manifest format (Rust)
  mortar-rumble/          rumble motor helper (Rust)
  m8c/, pico8/, trimui-brick-firmware.nix, mortar-gpu-tests/
  choochootracker/        ChooChooTracker (MIT tracker/groovebox), Mortar build + launcher
firmware/mainline/        U-Boot SPL + TF-A + U-Boot, with jkingston's patches
image/build-image-mainline.nix   SD image (bootloader at 128 KiB, FAT /boot, ext4 /)
docs/                     mortar.md, firmware.md, setup-file.md, pairing.md, updates.md
tests/mortar-update.nix   VM test: publish, download, stage, roll back (checks.*.mortar-update)
.github/workflows/        CI: build on push and publish for over-the-air updates
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
