# SPDX-License-Identifier: GPL-2.0-or-later
{ lib, rustPlatform }:

rustPlatform.buildRustPackage {
  pname = "mortar-rumble";
  version = "0.1.0";
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ];
  };
  cargoLock.lockFile = ./Cargo.lock;
  meta = {
    description = "Buzz the TrimUI Brick's rumble motor";
    license = lib.licenses.gpl2Plus;
    mainProgram = "mortar-rumble";
    platforms = lib.platforms.linux;
  };
}
