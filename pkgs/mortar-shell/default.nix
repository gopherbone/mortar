# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-shell: Mortar's controller-driven Wayland compositor (Smithay).
# Shares the UI toolkit, theme and input mapping with mortar-ui, and embeds
# the Wi-Fi / Remote access screens from mortar-setup (path dependencies,
# libraries only). See DESIGN.md.
#
#   nix build ... pkgs.callPackage ./pkgs/mortar-shell { }
#   passthru.tests.headless   compositor on the headless backend with real
#                             clients; screenshots in $out/*.png
{
  lib,
  rustPlatform,
  callPackage,
  runCommand,
  pkg-config,
  seatd,
  libgbm,
  pixman,
  libxkbcommon,
}:

let
  fonts = callPackage ../mortar-ui/fonts.nix { };
  shell = rustPlatform.buildRustPackage {
    pname = "mortar-shell";
    version = "0.1.0";

    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        ./Cargo.toml
        ./Cargo.lock
        ./src
        ../mortar-ui/Cargo.toml
        ../mortar-ui/src
        ../mortar-setup/Cargo.toml
        ../mortar-setup/src
        ../mortar-pair/Cargo.toml
        ../mortar-pair/src
      ];
    };
    sourceRoot = "source/mortar-shell";
    cargoLock.lockFile = ./Cargo.lock;

    nativeBuildInputs = [ pkg-config ];
    buildInputs = [ seatd libgbm pixman libxkbcommon ];

    env.MORTAR_UI_FONT_DIR = "${fonts}/share/fonts/mortar-ui";

    passthru = {
      inherit fonts;
      tests.headless-run = callPackage ./test.nix { mortar-shell = shell; };
      tests.headless = runCommand "mortar-shell-headless-check" { } ''
        r=${shell.tests.headless-run}
        cat $r/result
        grep -qx PASS $r/result
        ln -s $r $out
      '';
    };

    meta = {
      description = "Controller-driven Wayland compositor for Mortar on the TrimUI Brick";
      license = lib.licenses.gpl2Plus;
      platforms = lib.platforms.linux;
      mainProgram = "mortar-shell";
    };
  };
in
shell
