# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-packages: installing more from the Brick. Two programs from one crate:
#
#   mortar-packages   the app (a Wayland app for mortar-shell, in Files' language,
#                     software rendered with mortar-ui). See DESIGN.md.
#   mortar-packagesd  the root service it asks, on a socket (modules/core/packages.nix)
#
#   the cargo tests         the service's parts, the app driven with key events
#   passthru.tests.headless mortar-shell on its headless backend finds the app by its
#                           manifest and runs it against a made-up service: keys, the
#                           requests it sends, no frames at rest; screenshots in $out
#   tests/mortar-packages.nix  the service on a real Nix store (checks.*.mortar-packages)
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
  packages = rustPlatform.buildRustPackage {
    pname = "mortar-packages";
    version = "0.1.0";

    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        ./Cargo.toml
        ./Cargo.lock
        ./src
        ../mortar-index/Cargo.toml
        ../mortar-index/src
        ../mortar-update/Cargo.toml
        ../mortar-update/src
        ../mortar-files/Cargo.toml
        ../mortar-files/src
        ../mortar-files/tests
        ../mortar-ui/Cargo.toml
        ../mortar-ui/src
      ];
    };
    sourceRoot = "source/mortar-packages";
    cargoLock.lockFile = ./Cargo.lock;

    env.MORTAR_UI_FONT_DIR = "${fonts}/share/fonts/mortar-ui";

    passthru = {
      inherit fonts;
      tests.headless-run = callPackage ./test.nix {
        inherit mortar-shell;
        app = import ./app.nix {
          inherit pkgs;
          mkApp = import ../../lib/mk-app.nix { inherit lib; };
        };
      };
      tests.headless = runCommand "mortar-packages-headless-check" { } ''
        r=${packages.tests.headless-run}
        cat $r/result
        grep -qx PASS $r/result
        ln -s $r $out
      '';
    };

    meta = {
      description = "Install software on a TrimUI Brick from the Brick, with the buttons";
      license = lib.licenses.gpl2Plus;
      platforms = lib.platforms.linux ++ lib.platforms.darwin;
      mainProgram = "mortar-packages";
    };
  };
in
packages
