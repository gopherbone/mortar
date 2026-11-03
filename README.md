# Mortar

NixOS for the **TrimUI Brick** (TG3040, Allwinner A133): a mainline kernel and boot
chain, and a controller-driven multitasking shell. It boots from a microSD card and
leaves the stock OS on the internal eMMC alone: pull the card and the Brick boots
stock again.

What's on it is up to you. The hardware, the shell and each kind of app are separate
layers, and a flake of your own picks the ones it wants. What the shell does and how
it's used: [docs/using.md](docs/using.md).

## Layers

| Layer | Where | Flake output | What |
|---|---|---|---|
| Hardware | `devices/brick/` (sources: `firmware/mainline`, `dtb`, `patches`, `pkgs/linux-trimui-brick.nix`, ...) | `nixosModules.brick` | Kernel, boot chain, firmware, GPU, PMIC, panel, radios, SD image. Works without the UI. |
| Core | `modules/core/`, `pkgs/mortar-*` | `nixosModules.core` | Shell, greeter, setup, pairing, power, updates, the session user, the `mortar` command. |
| Packs | `modules/packs/` | `nixosModules.pack-emulation`, `pack-music`, `pack-ai`, `pack-terminal` | mortar-play; m8c, ChooChoo, Little XM; Claude Code; zsh. Importing one turns it on. |
| Ports | `modules/ports/` | `nixosModules.port-pico8`, `port-acgc`, `port-openxcom`, `port-puzzlescript` | Apps that need your own data. Each entry waits for it, or says where it goes. |
| Systems | `configurations/` | `nixosConfigurations.brick-bare`, `brick-full` | Device + core only, or everything. |

`modules/device.nix` is what the layers above ask of a device (`mortar.device`).

## Build a bare image

You need [Nix](https://nixos.org/download) with flakes and a way to build
`aarch64-linux` (a native machine, or on macOS `nixpkgs#darwin.linux-builder`).

```sh
nix build .#sd-image                 # bare; .#sd-image-full for everything
zstd -d result/sd-image/*.img.zst -o mortar.img
# write mortar.img to a microSD card (macOS helper: scripts/flash-mac.sh)
```

The build downloads TrimUI's public firmware release for its blobs, which aren't
redistributable, so the image isn't either: **never publish a built image or push it
to a public cache.** The same goes for emulator cores with non-commercial licences.
`mortar.unfree` lists what a system may use; `docs/updates.md` covers private updates.

After booting a system, change the configuration and:

```sh
nix run .#mortar -- pair                                # adds your SSH key
nix run .#mortar -- deploy --reboot --config brick-bare # --flake FLAKE for your own
```

## Your own system

```nix
{
  inputs.mortar.url = "github:gopherbone/mortar";

  outputs = { mortar, ... }: {
    nixosConfigurations.brick = mortar.lib.mkSystem {
      packs = [ mortar.nixosModules.pack-music ];
      ports = [ mortar.nixosModules.port-openxcom ];
      modules = [ ./my-apps.nix ];
    };
  };
}
```

### Add a pack

A pack is a module under `modules/packs/`, exported in `flake.nix`. To use one, add
it to `packs` as above. Turn parts of one off with their options, such as
`mortar.apps.littlexm.enable = false`.

### Write your own app

An app is a launcher entry, usually in a small module of your own:

```nix
{ config, pkgs, ... }:

{
  mortar.shell.apps.htop = {
    name = "htop";
    icon = "gear";
    gamepad = "keyboard";
    order = 50;
    command = config.mortar.shell.terminal.command ++ [ "${pkgs.htop}/bin/htop" ];
  };
}
```

SDL apps that should use the GPU start through
`${config.mortar.device.sdlRun}/bin/mortar-sdl-run`. For an unfree package, add its
name to `mortar.unfree`. `modules/ports/puzzlescript.nix` is a short real example;
`mortar.shell.apps` in `modules/core/shell.nix` lists the options.

## Game data never goes in the repository

Not game files, disc images, firmware dumps or ROMs: not in this repository, nor the
Nix store, CI or a cache. Ports hold only the engine; your data goes straight to the
device (the store would make it world-readable and cacheable):

| Port | Your data | How |
|---|---|---|
| PICO-8 | your Raspberry Pi zip | `nix-store --add-fixed sha256 pico-8_0.2.7_raspi.zip`, then `mortar pico8` installs it in its own profile (see pkgs/pico8) |
| Animal Crossing | your disc image | `mortar acgc --iso PATH` |
| OpenXcom | your UFO: Enemy Unknown files | `mortar xcom --data PATH` |
| PuzzleScript | `.txt` games | copy to `~/.local/share/puzzlescript/games` |

Keep it that way in anything you contribute.

## What has and hasn't been tested

Tested on one Brick, by its owner: the system the full configuration describes,
with all its apps (details and open items in [docs/using.md](docs/using.md)). Its
store path has been compared with the one before this layout: it differs only in the
PuzzleScript engine, and in Redemption: Liar leaving (it lives in its own repository).

Built and evaluated, **never booted**: the bare system and its SD image. The
fresh-card path as a whole (write an image, first boot, grow the root partition,
setup, pair) has not been retested since the system stopped using an initrd, so treat
it as unproven. Keep your data elsewhere and expect to debug over the serial console.

Evaluated only: each pack and port on its own, a flake outside this repository using
`mkSystem`, and core with no device module. Over-the-air updates have a VM test
(`checks.*.mortar-update`) but have not been run on a device.

Only the Brick exists as a device. `modules/device.nix` is a first cut, and the shell
still assumes more than it covers: a 1024x768 panel and Brick button codes in Rust,
`gpio-vibrator` for rumble, `usb0` and `wlan0`, and the card name `Codec` when PipeWire
is off.

The earlier setup (`firmware/`'s vendor-boot0 chain, `re_tools/`, `harness/`,
`FINDINGS.md`) is brittonr's work, kept for reference; Mortar doesn't boot it.

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
- Stephen Lavelle for [PuzzleScript](https://github.com/increpare/PuzzleScript):
  `pkgs/puzzlescript` derives from it, as an MIT-licensed port (see its README).
- laamaa (m8c), Lexaloffle (PICO-8), the libretro core authors, and
  Butterfly's Pond website for the visual inspiration. The Claude Spark is a
  trademark of Anthropic, used to label the Claude app.

## License

GPL-2.0-or-later for code in this repository (see `LICENSE`), except
`pkgs/puzzlescript`, which is MIT (its own `LICENSE`). Patches keep their upstream
licenses. The TrimUI binaries under `firmware/` and `re_tools/` are covered by neither
(see `LICENSE`).
