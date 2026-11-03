# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-ai: controller-friendly Claude Code front end (a Wayland app for
# mortar-shell) and the sign-in helper (`mortar-ai pair`). Pure Rust,
# software rendered with the mortar-ui toolkit (path dependency, library
# only). See DESIGN.md; the NixOS side is modules/packs/ai.nix.
#
#   passthru.tests.headless   mortar-shell on its headless backend runs
#                             mortar-ai against a fake `claude -p` and the
#                             pair page against curl; screenshots in $out
{
  lib,
  rustPlatform,
  callPackage,
  runCommand,
  mortar-shell ? callPackage ../mortar-shell { },
}:

let
  fonts = callPackage ../mortar-ui/fonts.nix { };
  ai = rustPlatform.buildRustPackage {
    pname = "mortar-ai";
    version = "0.1.0";

    src = lib.fileset.toSource {
      root = ../.;
      fileset = lib.fileset.unions [
        ./Cargo.toml
        ./Cargo.lock
        ./src
        ../mortar-ui/Cargo.toml
        ../mortar-ui/src
      ];
    };
    sourceRoot = "source/mortar-ai";
    cargoLock.lockFile = ./Cargo.lock;

    env.MORTAR_UI_FONT_DIR = "${fonts}/share/fonts/mortar-ui";

    passthru = {
      inherit fonts;
      tests.headless-run = callPackage ./test.nix { mortar-ai = ai; inherit mortar-shell; };
      tests.headless = runCommand "mortar-ai-headless-check" { } ''
        r=${ai.tests.headless-run}
        cat $r/result
        grep -qx PASS $r/result
        ln -s $r $out
      '';
    };

    meta = {
      description = "Controller-friendly Claude Code front end for Mortar on the TrimUI Brick";
      license = lib.licenses.gpl2Plus;
      platforms = lib.platforms.linux ++ lib.platforms.darwin;
      mainProgram = "mortar-ai";
    };
  };
in
ai
