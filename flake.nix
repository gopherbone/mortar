# SPDX-License-Identifier: GPL-2.0-or-later
{
  description = "Mortar: a NixOS distro for the TrimUI Brick";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      lib = nixpkgs.lib;
      linuxSystems = [ "x86_64-linux" "aarch64-linux" ];
      allSystems = linuxSystems ++ [ "aarch64-darwin" "x86_64-darwin" ];
      full = self.nixosConfigurations.brick-full.config;
      bare = self.nixosConfigurations.brick-bare.config;
    in {
      # Build your own system: device + core + these packs + your modules.
      lib = import ./lib { inherit self nixpkgs; };

      nixosModules = {
        # Hardware. Works without the rest (a console system).
        brick = ./devices/brick;
        # What a device module sets for the layers above.
        device-interface = ./modules/device.nix;

        # The shell, greeter, setup and pairing, power, updates, users.
        core = ./modules/core;
        default = self.nixosModules.core;

        # Importing a pack or a port turns it on.
        pack-emulation = ./modules/packs/emulation.nix; # mortar-play
        pack-music = ./modules/packs/music; # m8c, ChooChoo, Little XM
        pack-ai = ./modules/packs/ai.nix; # Claude Code; in no configuration here
        pack-terminal = ./modules/packs/terminal.nix; # zsh

        # Bring your own data: nothing of the game is ever in this repository.
        port-puzzlescript = ./modules/ports/puzzlescript.nix;
        port-pico8 = ./modules/ports/pico8.nix;
        port-acgc = ./modules/ports/acgc.nix;

        full = ./configurations/full.nix;
      };

      nixosConfigurations = {
        # Every pack and port that may be published (no Claude Code, no
        # non-commercial cores).
        brick-full = self.lib.mkSystem { modules = [ ./configurations/full.nix ]; };
        # The old name of brick-full.
        brick-mainline = self.nixosConfigurations.brick-full;
        # Boot, setup, Wi-Fi, updates and the shell, no packs or ports.
        brick-bare = self.lib.mkSystem { modules = [ ./configurations/bare.nix ]; };
      };

      packages = lib.genAttrs allSystems (system:
        let
          pkgs = import nixpkgs {
            inherit system;
            config.allowUnfreePredicate = pkg: builtins.elem (lib.getName pkg) full.mortar.unfree;
          };
        in {
          # SD images (aarch64-linux; embed TrimUI's SCP blob, see docs/updates.md).
          sd-image = bare.system.build.sdImage;
          sd-image-full = full.system.build.sdImage;
          default = self.packages.${system}.sd-image;

          # The Brick's kernel and boot chain
          linux-trimui-brick = full.boot.kernelPackages.kernel;
          u-boot-mainline = full.system.build.mortarBootloader;

          mortar-toplevel = full.system.build.toplevel;
          bare-toplevel = bare.system.build.toplevel;

          # Not in the system: `mortar pico8` builds this and installs it
          # into /nix/var/nix/profiles/pico8 on the Brick.
          pico8 = full.mortar.apps.pico8.package;
          # Not in the system either: `mortar acgc` installs it into
          # /nix/var/nix/profiles/acgc (no game data; pkgs/acgc).
          acgc = full.mortar.apps.acgc.package;
          # An app with a manifest (lib.mkApp): `mortar xcom` installs it into
          # /nix/var/nix/profiles/apps/openxcom (no game data; pkgs/openxcom).
          openxcom = self.nixosConfigurations.brick-full.pkgs.callPackage ./pkgs/openxcom/app.nix {
            inherit (self.lib) mkApp;
          };

          # tools/showcase: frames of the real shell for the showcase video
          showcase-capture = pkgs.callPackage ./tools/showcase/capture.nix { };
        });

      # `nix run .#mortar -- deploy` from a Mac or Linux machine
      apps = lib.genAttrs allSystems (system:
        let
          pkgs = nixpkgs.legacyPackages.${system};
          mortar = pkgs.callPackage ./pkgs/mortar-cli { };
        in {
          mortar = { type = "app"; program = "${mortar}/bin/mortar"; };
          # tools/showcase: compose the showcase video from showcase-capture
          showcase = { type = "app"; program = "${pkgs.callPackage ./tools/showcase/compose.nix { }}/bin/mortar-showcase"; };
          default = self.apps.${system}.mortar;
        });

      # `nix build .#checks.aarch64-linux.NAME`. mortar-update and usb-pairing are
      # NixOS VM tests: they need KVM (docs/updates.md, Runners).
      checks = lib.genAttrs linuxSystems (system:
        let pkgs = nixpkgs.legacyPackages.${system};
        in {
          # over-the-air updates end to end (Garage as the bucket)
          mortar-update = import ./tests/mortar-update.nix { inherit pkgs; };
          # apps in their own profiles on a real Nix store
          app-profiles = import ./tests/app-profiles.nix { inherit pkgs; inherit (self.lib) mkApp; };
          # apps appearing and leaving in a running shell (pkgs/mortar-shell)
          mortar-shell-discovery = (pkgs.callPackage ./pkgs/mortar-shell { }).tests.discovery;
          # the engine without bundled games (pkgs/puzzlescript)
          puzzlescript = pkgs.callPackage ./pkgs/puzzlescript/check.nix {
            puzzlescript = pkgs.callPackage ./pkgs/puzzlescript { };
          };
          # pairing, SSH with a key and nix copy over an untrusted usb0
          usb-pairing = import ./tests/usb-pairing.nix { inherit pkgs; };
          # what a host on the USB cable gets (docs/pairing.md)
          usb-access = pkgs.callPackage ./tests/usb-access.nix {
            closed = bare;
            open = (self.lib.mkSystem {
              modules = [ ./configurations/bare.nix { mortar.usb.trustHost = true; } ];
            }).config;
          };
          # the compositor, mortar-play and mortar-ai headless, with real clients
          # (mortar-ai against a stand-in for `claude`: no Claude Code in here)
          mortar-shell-headless = (pkgs.callPackage ./pkgs/mortar-shell { }).tests.headless;
          mortar-play-integration = (pkgs.callPackage ./pkgs/mortar-play { }).tests.integration;
          mortar-play-shell = (pkgs.callPackage ./pkgs/mortar-play { }).tests.shell;
          mortar-ai-headless = (pkgs.callPackage ./pkgs/mortar-ai {
            mortar-shell = pkgs.callPackage ./pkgs/mortar-shell { };
          }).tests.headless;

          # every pack and port on its own evaluates, and what's published holds
          # nothing it shouldn't (docs/updates.md)
          eval-matrix = pkgs.writeText "mortar-eval-matrix"
            (builtins.unsafeDiscardStringContext (builtins.toJSON self.lib.evalMatrix));
          public-policy = pkgs.callPackage ./tests/public-policy.nix { inherit self; };
        } // lib.optionalAttrs (system == "aarch64-linux") {
          # which generation U-Boot's rollback key boots (devices/brick/rollback.nix)
          previous-conf = self.nixosConfigurations.brick-bare.pkgs.callPackage ./tests/previous-conf.nix {
            previousConf = bare.system.build.mortarPreviousConf;
          };
          # the move from root's session, with and without the AI pack
          user-migrate = self.nixosConfigurations.brick-bare.pkgs.callPackage ./tests/user-migrate.nix {
            coreMigrate = bare.system.build.mortarUserMigrate;
            aiMigrate = (self.lib.mkSystem {
              modules = [ ./configurations/bare.nix self.nixosModules.pack-ai ];
            }).config.system.build.mortarUserMigrate;
          };
        });

      devShells = lib.genAttrs linuxSystems (system:
        let pkgs = import nixpkgs { inherit system; };
        in {
          default = pkgs.mkShell {
            buildInputs = with pkgs; [
              dtc dosfstools e2fsprogs mtools parted util-linux zstd file python3
            ];
            shellHook = ''
              echo "Mortar dev shell"
              echo "  nix build .#sd-image       - bare SD card image"
              echo "  nix build .#sd-image-full  - everything"
            '';
          };
        }
      );
    };
}
