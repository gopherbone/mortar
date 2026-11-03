# SPDX-License-Identifier: GPL-2.0-or-later
# Build a system from Mortar's layers:
#
#   mortar.lib.mkSystem {
#     packs = [ mortar.nixosModules.pack-music ];
#     modules = [ ./my-apps.nix ];
#   }
{ self, nixpkgs }:

{
  mkSystem =
    {
      device ? self.nixosModules.brick,
      packs ? [ ],
      ports ? [ ],
      modules ? [ ],
      system ? "aarch64-linux",
    }:
    nixpkgs.lib.nixosSystem {
      inherit system;
      modules = modules ++ [ device self.nixosModules.core ] ++ packs ++ ports;
    };
}
