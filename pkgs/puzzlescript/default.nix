# SPDX-License-Identifier: MIT
# PuzzleScript: the native C engine (src/) with its SDL3 shell (sdl/). See
# README.md. No games ship with it: they are the user's own .txt files.
{ lib, rustPlatform, pkg-config, sdl3 }:

rustPlatform.buildRustPackage {
  pname = "puzzlescript-sdl";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [ ./LICENSE ./src ./sdl ];
  };

  cargoRoot = "sdl";
  buildAndTestSubdir = "sdl";
  cargoLock.lockFile = ./sdl/Cargo.lock;

  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ sdl3 ];

  meta = {
    description = "Native C PuzzleScript engine with an SDL3 front end";
    license = lib.licenses.mit;
    mainProgram = "puzzlescript";
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
  };
}
