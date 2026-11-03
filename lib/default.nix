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

let
  inherit (nixpkgs) lib;
  bare = ../configurations/bare.nix;
in
{
  mkApp = import ./mk-app.nix { inherit (nixpkgs) lib; };

  # bare, full, and each pack and port alone on top of bare.
  matrix =
    {
      bare = self.nixosConfigurations.brick-bare;
      full = self.nixosConfigurations.brick-full;
    }
    // lib.mapAttrs (_: m: self.lib.mkSystem { modules = [ bare m ]; }) (
      lib.filterAttrs (n: _: lib.hasPrefix "pack-" n || lib.hasPrefix "port-" n) self.nixosModules
    );

  # Evaluates them all, building nothing: `nix eval --json .#lib.evalMatrix`
  evalMatrix = lib.mapAttrs (_: s: s.config.system.build.toplevel.drvPath) self.lib.matrix;

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
