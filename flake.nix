# SPDX-License-Identifier: GPL-2.0-or-later
{
  description = "NixOS for TrimUI Brick (Allwinner A133) — mainline U-Boot + mainline kernel";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    # PuzzleScript (private repo; fetched over SSH with the user's key)
    puzzlescript = {
      url = "git+ssh://git@github.com/gopherbone/puzzlescript_pd?ref=sdl";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, puzzlescript }:
    let
      supportedSystems = [ "x86_64-linux" "aarch64-linux" ];
      forAllSystems = nixpkgs.lib.genAttrs supportedSystems;
      targetSystem = "aarch64-linux";
      # Vendor SCP blob (not redistributable) and the U-Boot image embedding it.
      # Build locally only; never push these to a public cache.
      mortarUnfree = pkg: builtins.elem (nixpkgs.lib.getName pkg)
        [ "trimui-brick-scp" "u-boot-trimui-brick-mainline" "trimui-brick-firmware" "pvr-ddk-userspace" "claude-code" "pico-8" "pico" "acgc" "liar"
          # mortar-play cores with non-commercial licences
          "libretro-snes9x2005-plus" "libretro-genesis-plus-gx" "libretro-fbneo" ];
    in {
      nixosConfigurations.trimui-brick = nixpkgs.lib.nixosSystem {
        system = targetSystem;
        modules = [
          ./modules/trimui-brick.nix
          ./configuration.nix
          ({ lib, ... }: {
            nixpkgs.hostPlatform = lib.mkDefault "aarch64-linux";
          })
        ];
      };

      # Mortar: jkingston's mainline kernel (Linux 7.3) + native Brick DTB,
      # USB gadget console/network, black-box logs. Meant for the fully
      # mainline SPL + TF-A + U-Boot chain (packaged separately).
      nixosConfigurations.brick-mainline = nixpkgs.lib.nixosSystem {
        system = targetSystem;
        modules = [
          ./modules/trimui-brick.nix
          ./modules/mortar-usb.nix
          ./modules/mortar-usb-watchdog.nix
          ./modules/brick-firmware.nix
          ./modules/brick-gpu.nix
          ./modules/mortar-apps.nix
          {
            # The patch is for upstream (the puzzlescript repo); see the file.
            mortar.apps.puzzlescript.package = puzzlescript.packages.aarch64-linux.default.overrideAttrs (old: {
              patches = (old.patches or [ ]) ++ [ ./pkgs/puzzlescript/kmsdrm-redraw.patch ];
            });
          }
          ./modules/mortar-greeter.nix
          ./modules/mortar-shell.nix
          {
            mortar.shell.enable = true;
            # boot straight into the shell (the greeter after Log out)
            mortar.shell.autologin.user = "brick";
          }
          ./modules/mortar-ai.nix
          { mortar.ai.enable = true; }
          ./modules/mortar-setup.nix
          { mortar.setup.enable = true; }
          ./modules/mortar-play.nix
          { mortar.play.enable = true; }
          ./modules/mortar-power.nix
          { mortar.power.enable = true; }
          ({ config, lib, pkgs, ... }: {
            mortar.apps.pico8.enable = true;
            # Animal Crossing: like PICO-8, installed by `mortar acgc --iso`
            # with the user's own disc; the entry hides until the ISO is there.
            mortar.apps.acgc.enable = true;
            mortar.shell.categories.acgc = "games";
            # Redemption: Liar: installed by `mortar liar --data` with the
            # user's own files from their card; hidden until they're there.
            mortar.apps.liar.enable = true;
            mortar.shell.categories.liar = "games";
            mortar.shell.categories.littlexm = "music";
            mortar.shell.categories.openxcom = "games";
            mortar.greeter.enable = true;
            mortar.greeter.sessions = {
              games = {
                name = "PuzzleScript";
                description = "Puzzle games, controller";
                icon = "pad";
                order = 10;
                command = "exec ${config.mortar.gpu.sdlRun}/bin/mortar-sdl-run ${config.mortar.apps.puzzlescript.package}/bin/puzzlescript";
              };
              m8 = {
                name = "M8";
                description = "Tracker on the top USB-C port";
                icon = "usb";
                order = 20;
                command = "exec ${config.mortar.gpu.sdlRun}/bin/mortar-sdl-run ${config.mortar.apps.m8c.package}/bin/m8c-brick";
              };
              choochoo = {
                name = "ChooChoo";
                description = "Tracker · groovebox";
                icon = "pad";
                order = 22;
                command = "exec ${config.mortar.gpu.sdlRun}/bin/mortar-sdl-run ${config.mortar.apps.choochootracker.package}/bin/choochootracker";
              };
              littlexm = lib.mkIf config.mortar.apps.littlexm.enable {
                name = "Little XM";
                description = "Tracker · FastTracker XM";
                icon = "pad";
                order = 23;
                command = "exec ${config.mortar.gpu.sdlRun}/bin/mortar-sdl-run ${config.mortar.apps.littlexm.package}/bin/little-xm-tracker";
              };
              # PICO-8 is not in the system closure: `mortar pico8` installs
              # it into its own profile; the entry hides until then.
              pico8 = {
                name = "PICO-8";
                description = "Fantasy console, Splore";
                icon = "pad";
                order = 25;
                requires = config.mortar.apps.pico8.bin;
                command = "exec ${config.mortar.gpu.sdlRun}/bin/mortar-sdl-run ${config.mortar.apps.pico8.bin} -splore";
              };
              openxcom = lib.mkIf config.mortar.apps.openxcom.enable {
                name = "OpenXcom";
                description = "UFO Defense · your own copy";
                icon = "pad";
                order = 27;
                requires = config.mortar.apps.openxcom.data;
                command = "exec ${config.mortar.gpu.sdlRun}/bin/mortar-sdl-run ${config.mortar.apps.openxcom.package}/bin/openxcom";
              };
              acgc = lib.mkIf config.mortar.apps.acgc.enable {
                name = "Animal Crossing";
                description = "GameCube · your own disc";
                icon = "pad";
                order = 26;
                requires = config.mortar.apps.acgc.rom;
                command = "exec ${config.mortar.gpu.sdlRun}/bin/mortar-sdl-run ${config.mortar.apps.acgc.bin}";
              };
              liar = lib.mkIf config.mortar.apps.liar.enable {
                name = "Redemption: Liar";
                description = "GP2X Wiz · your own copy";
                icon = "pad";
                order = 28;
                requires = config.mortar.apps.liar.data;
                # No mortar-sdl-run: a 32-bit process, no GPU, its own Wayland client.
                command = "exec ${config.mortar.apps.liar.bin}";
              };
              terminal = {
                name = "Terminal";
                description = "Console";
                icon = "term";
                order = 30;
                command = "exec ${pkgs.bashInteractive}/bin/bash --login";
              };
            };
          })
          ./modules/mortar-base.nix
          ./modules/mortar-fastboot.nix
          { mortar.fastboot.enable = true; }
          # Over-the-air updates from a private binary cache (docs/updates.md).
          # Nothing here is secret: bucket, endpoint, the public signing key.
          # Fill these in, set enable = true and `mortar deploy` once; after
          # that CI (or `mortar publish`) updates the Brick over Wi-Fi.
          # `mortar publish` reads these values too.
          ./modules/mortar-update.nix
          {
            mortar.update = {
              enable = false;
              bucket = "mortar-cache";
              endpoint = "https://ACCOUNT_ID.r2.cloudflarestorage.com";
              publicKey = "mortar-1:REPLACE_WITH_PUBLIC_KEY";
            };
          }
          ./modules/mortar-user.nix
          ./modules/mortar-zsh.nix
          # SD image: mainline SPL + TF-A + U-Boot at 128 KiB, FAT /boot
          # with extlinux generations, ext4 root.
          (import ./image/build-image-mainline.nix { })
          ({ lib, ... }: {
            nixpkgs.hostPlatform = lib.mkDefault "aarch64-linux";
            nixpkgs.config.allowUnfreePredicate = mortarUnfree;
            hardware.trimui-brick.variant = "mainline";
            # TF-A /reserved-memory (0x40000000, 0x40000, no-map)
            hardware.trimui-brick.extraDtOverlays = [
              { name = "tf-a-reserved"; dtsFile = ./firmware/mainline/reserved-memory.dtso; }
            ];
            # FAT /boot is 512 MiB; keep a handful of generations
            boot.loader.generic-extlinux-compatible.configurationLimit = 5;
          })
        ];
      };

      packages = forAllSystems (system:
        let
          pkgs = import nixpkgs { inherit system; config.allowUnfreePredicate = mortarUnfree; };
          nixosConfig = self.nixosConfigurations.trimui-brick.config;
          mainlineConfig = self.nixosConfigurations.brick-mainline.config;

          u-boot = pkgs.callPackage ./firmware/u-boot.nix {};
          bootPackage = pkgs.callPackage ./firmware/boot-package.nix {
            u-boot-trimui-brick = u-boot;
          };
        in {
          inherit u-boot;
          boot-package = bootPackage;

          sd-image = import ./image/build-image.nix {
            inherit pkgs bootPackage;
            lib = nixpkgs.lib;
            config = nixosConfig;
          };

          write-sd = pkgs.writeShellScriptBin "write-trimui-sd" ''
            set -euo pipefail
            if [ $# -ne 1 ]; then
              echo "Usage: write-trimui-sd /dev/sdX"
              echo "WARNING: This will overwrite all data on the device!"
              exit 1
            fi
            DEVICE="$1"
            if [ ! -b "$DEVICE" ]; then
              echo "Error: $DEVICE is not a block device"
              exit 1
            fi
            IMAGE="${self.packages.${system}.sd-image}/nixos-trimui-brick.img"
            if [ ! -f "$IMAGE" ]; then
              IMAGE="${self.packages.${system}.sd-image}/nixos-trimui-brick.img.zst"
            fi
            if [ ! -f "$IMAGE" ]; then
              echo "Error: Could not find SD image"
              exit 1
            fi
            echo "Writing $IMAGE to $DEVICE..."
            echo "This will DESTROY all data on $DEVICE!"
            read -p "Are you sure? (yes/no) " confirm
            if [ "$confirm" != "yes" ]; then
              echo "Aborted."
              exit 1
            fi
            if [[ "$IMAGE" == *.zst ]]; then
              ${pkgs.zstd}/bin/zstd -d -c "$IMAGE" | dd of="$DEVICE" bs=4M status=progress
            else
              dd if="$IMAGE" of="$DEVICE" bs=4M status=progress
            fi
            sync
            echo "Done! Insert SD card into TrimUI Brick and power on."
          '';

          # Mortar kernel (always aarch64-linux, from the NixOS config)
          linux-trimui-brick = mainlineConfig.boot.kernelPackages.kernel;
          mortar-toplevel = mainlineConfig.system.build.toplevel;
          # Not in the system: `mortar pico8` builds this and installs it
          # into /nix/var/nix/profiles/pico8 on the Brick.
          pico8 = mainlineConfig.mortar.apps.pico8.package;
          # Not in the system either: `mortar acgc` installs it into
          # /nix/var/nix/profiles/acgc (no game data; pkgs/acgc).
          acgc = mainlineConfig.mortar.apps.acgc.package;
          # Same for Redemption: Liar: `mortar liar` (no game data; pkgs/liar).
          liar = mainlineConfig.mortar.apps.liar.package;
          # The engine alone, for testing builds on the Brick.
          openxcom = mainlineConfig.mortar.apps.openxcom.package;

          # Mainline boot chain + SD image (fully mainline SPL/TF-A/U-Boot)
          u-boot-mainline = mainlineConfig.system.build.mortarBootloader;
          sd-image-mainline = mainlineConfig.system.build.sdImage;

          # tools/showcase: frames of the real shell for the showcase video
          showcase-capture = pkgs.callPackage ./tools/showcase/capture.nix { };

          default = self.packages.${system}.sd-image;
        }
      );

      # `nix run .#mortar -- deploy` from a Mac or Linux machine
      apps = nixpkgs.lib.genAttrs [ "aarch64-darwin" "x86_64-darwin" "x86_64-linux" "aarch64-linux" ] (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          mortar = pkgs.callPackage ./pkgs/mortar-cli { };
        in {
          mortar = { type = "app"; program = "${mortar}/bin/mortar"; };
          # tools/showcase: compose the showcase video from showcase-capture
          showcase = { type = "app"; program = "${pkgs.callPackage ./tools/showcase/compose.nix { }}/bin/mortar-showcase"; };
          default = self.apps.${system}.mortar;
        });

      # VM tests: `nix build .#checks.aarch64-linux.mortar-update`
      checks = forAllSystems (system: {
        # over-the-air updates end to end (MinIO as the bucket)
        mortar-update = import ./tests/mortar-update.nix { pkgs = nixpkgs.legacyPackages.${system}; };
      });

      devShells = forAllSystems (system:
        let pkgs = import nixpkgs { inherit system; };
        in {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [
              dtc dosfstools e2fsprogs mtools parted util-linux zstd file python3
              git-lfs
            ];
            shellHook = ''
              git-lfs install --local > /dev/null 2>&1 || true
              echo "TrimUI Brick NixOS dev shell"
              echo "  nix build .#sd-image       - Build complete SD card image"
              echo "  nix build .#u-boot         - Build U-Boot only"
              echo "  nix build .#boot-package   - Build boot_package only"
              echo "  nix run .#write-sd         - Write image to SD card"
            '';
          };
        }
      );
    };
}
