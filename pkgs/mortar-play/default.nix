# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-play: minimal, preconfigured libretro frontend (Rust). Library UI
# and in-game menu from the mortar-ui toolkit (path dependency, library
# only); SDL3 for audio, gamepads and the window outside mortar-shell; a
# plain wl_shm client inside it. See DESIGN.md.
#
#   passthru.tests.integration   headless: built-in NES test program on
#                                fceumm + the 2048 core, screenshots, audio,
#                                save states, SRAM, the UI flow ($out/*.png)
{
  lib,
  stdenv,
  rustPlatform,
  callPackage,
  runCommand,
  pkg-config,
  sdl3,
}:

let
  fonts = callPackage ../mortar-ui/fonts.nix { };
  play = rustPlatform.buildRustPackage {
    pname = "mortar-play";
    version = "0.1.0";

    src = lib.fileset.toSource {
      root = ../.;
      # test*.nix and dev.sh stay out of the source, so editing them doesn't
      # rebuild the frontend
      fileset = lib.fileset.unions [
        ./Cargo.toml
        ./Cargo.lock
        ./build.rs
        ./csrc
        ./src
        ../mortar-ui/Cargo.toml
        ../mortar-ui/src
      ];
    };
    sourceRoot = "source/mortar-play";
    cargoLock.lockFile = ./Cargo.lock;

    nativeBuildInputs = [ pkg-config ];
    buildInputs = [ sdl3 ];

    env.MORTAR_UI_FONT_DIR = "${fonts}/share/fonts/mortar-ui";

    passthru = {
      inherit fonts;
      tests.integration-run = callPackage ./test.nix { mortar-play = play; };
      tests.integration = runCommand "mortar-play-integration-check" { } ''
        r=${play.tests.integration-run}
        cat $r/result
        grep -qx PASS $r/result
        ln -s $r $out
      '';
      # inside mortar-shell (Linux only)
      tests.shell-run = callPackage ./test-shell.nix {
        mortar-play = play;
        mortar-shell = callPackage ../mortar-shell { };
      };
      tests.shell = runCommand "mortar-play-shell-check" { } ''
        r=${play.tests.shell-run}
        cat $r/result
        grep -qx PASS $r/result
        ln -s $r $out
      '';
    };

    meta = {
      description = "Minimal, preconfigured libretro frontend for Mortar on the TrimUI Brick";
      license = lib.licenses.gpl2Plus;
      platforms = lib.platforms.linux ++ lib.platforms.darwin;
      mainProgram = "mortar-play";
    };
  };
in
play
