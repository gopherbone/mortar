# SPDX-License-Identifier: GPL-2.0-or-later
{ lib, rustPlatform }:

rustPlatform.buildRustPackage {
  pname = "mortar-power";
  version = "0.1.0";
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [ ./Cargo.toml ./Cargo.lock ./src ];
  };
  cargoLock.lockFile = ./Cargo.lock;
  meta = {
    description = "Battery policy for the TrimUI Brick: USB input current, low-battery warnings, clean power-off";
    license = lib.licenses.gpl2Plus;
    mainProgram = "mortar-power";
    platforms = lib.platforms.linux;
  };
}
