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
        [ "trimui-brick-scp" "u-boot-trimui-brick-mainline" "trimui-brick-firmware" "pvr-ddk-userspace" "claude-code" ];
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
          { mortar.shell.enable = true; }
          ./modules/mortar-ai.nix
          { mortar.ai.enable = true; }
          ({ config, pkgs, ... }: {
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

          # Mainline boot chain + SD image (fully mainline SPL/TF-A/U-Boot)
          u-boot-mainline = mainlineConfig.system.build.mortarBootloader;
          sd-image-mainline = mainlineConfig.system.build.sdImage;

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
          default = self.apps.${system}.mortar;
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
