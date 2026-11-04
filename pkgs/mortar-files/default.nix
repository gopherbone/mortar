# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-files: a file manager for the Brick's buttons (a Wayland app for
# mortar-shell). Pure Rust, software rendered with the mortar-ui toolkit (path
# dependency, library only). See DESIGN.md. It's in the core system
# (`mortar.shell.files`); `app.nix` gives it a manifest for the headless test.
#
#   the cargo tests         the operations against real temp folders, and the
#                           app driven with key events
#   passthru.tests.headless mortar-shell on its headless backend finds the app
#                           by its manifest and runs it: keys, real file
#                           operations, the on-screen keyboard, no frames at
#                           rest; screenshots in $out
{
  lib,
  pkgs,
  rustPlatform,
  callPackage,
  runCommand,
  mortar-shell ? callPackage ../mortar-shell { },
}:

let
  fonts = callPackage ../mortar-ui/fonts.nix { };
  files = rustPlatform.buildRustPackage {
    pname = "mortar-files";
    version = "0.1.0";

    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        ./Cargo.toml
        ./Cargo.lock
        ./src
        ./tests
        ../mortar-ui/Cargo.toml
        ../mortar-ui/src
      ];
    };
    sourceRoot = "source/mortar-files";
    cargoLock.lockFile = ./Cargo.lock;

    env.MORTAR_UI_FONT_DIR = "${fonts}/share/fonts/mortar-ui";

    # the cross-disk tests say so if /dev/shm isn't a second filesystem here
    preCheck = ''
      echo "temp folder on device $(stat -c %d "''${TMPDIR:-/tmp}"), /dev/shm on $(stat -c %d /dev/shm)"
    '';

    passthru = {
      inherit fonts;
      tests.headless-run = callPackage ./test.nix {
        inherit mortar-shell;
        app = import ./app.nix {
          inherit pkgs;
          mkApp = import ../../lib/mk-app.nix { inherit lib; };
        };
      };
      tests.headless = runCommand "mortar-files-headless-check" { } ''
        r=${files.tests.headless-run}
        cat $r/result
        grep -qx PASS $r/result
        ln -s $r $out
      '';
    };

    meta = {
      description = "A file manager for Mortar on the TrimUI Brick, driven with the buttons";
      license = lib.licenses.gpl2Plus;
      platforms = lib.platforms.linux ++ lib.platforms.darwin;
      mainProgram = "mortar-files";
    };
  };
in
files
