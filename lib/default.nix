# SPDX-License-Identifier: GPL-2.0-or-later
# Build a system from Mortar's layers:
#
#   mortar.lib.mkSystem {
#     packs = [ mortar.nixosModules.pack-music ];
#     modules = [ ./my-apps.nix ];
#   }
#
# And an app the shell finds at runtime, outside any system (mk-app.nix):
#
#   mortar.lib.mkApp pkgs { package = ...; id = "my-port"; name = "My Port"; }
{ self, nixpkgs }:

{
  mkApp = import ./mk-app.nix { inherit (nixpkgs) lib; };

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
