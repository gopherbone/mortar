# SPDX-License-Identifier: GPL-2.0-or-later
# What may be published (docs/updates.md, modules/unfree.nix), at evaluation:
#
#   - bare and full evaluate to the same system with mortar.public on, which
#     refuses every unfree package outside mortar.publicUnfree. So nothing else
#     unfree is in them, however it got there.
#   - The opt-ins are in fact refused: Claude Code and a root console for USB hosts.
#   - The non-commercial cores are in, and can be turned off.
#   - A flake of your own doesn't follow the public channel by accident: it would
#     replace that system with the published one.
#
# `lib.evalMatrix` covers the rest; CI's closure check is the second line, by name,
# for what isn't marked unfree.
{ lib, runCommand, self }:

let
  drv = modules: (self.lib.mkSystem { inherit modules; }).config.system.build.toplevel.drvPath;
  public = { mortar.public = true; };
  follows = ../configurations/public.nix;
  bare = ../configurations/bare.nix;
  full = ../configurations/full.nix;
  refused = m: !(builtins.tryEval (builtins.deepSeq (drv m) true)).success;
  update = modules: (self.lib.mkSystem { inherit modules; }).config.mortar.update;
  # as a flake of your own builds on `full`
  own = [ self.nixosModules.full self.nixosModules.pack-ai ];

  checks = {
    "bare holds only what may be published" = drv [ bare follows public ] == self.nixosConfigurations.brick-bare.config.system.build.toplevel.drvPath;
    "full holds only what may be published" = drv [ full follows public ] == self.nixosConfigurations.brick-full.config.system.build.toplevel.drvPath;
    "Claude Code is refused" = refused [ bare self.nixosModules.pack-ai public ];
    "the non-commercial cores may be published" =
      !(refused [ bare self.nixosModules.pack-emulation public ]);
    "the non-commercial cores can be turned off" =
      !(refused [ bare self.nixosModules.pack-emulation { mortar.play.nonCommercialCores = false; } public ]);
    "a trusted USB host is refused" = refused [ bare { mortar.usb.trustHost = true; } public ];
    "both published systems follow the public cache" =
      lib.all (c: c.mortar.update.enable && c.mortar.update.baseUrl == "https://mortar.angelfire.workers.dev" && c.mortar.update.bucket == null) [
        self.nixosConfigurations.brick-bare.config
        self.nixosConfigurations.brick-full.config
      ];
    "each under its own name" =
      self.nixosConfigurations.brick-bare.config.mortar.update.configName == "brick-bare"
      && self.nixosConfigurations.brick-full.config.mortar.update.configName == "brick-full";
    "Packages follows Mortar's repository and no other, with the update's source and key" =
      lib.all (c:
        let rs = c.mortar.packages.repositories; in
        c.mortar.packages.enable
        && map (r: r.id) rs == [ "mortar" ]
        && (lib.head rs).url == c.mortar.update.baseUrl
        && (lib.head rs).publicKeys == c.mortar.update.trustedKeys
        && (lib.head rs).official
      ) [
        self.nixosConfigurations.brick-bare.config
        self.nixosConfigurations.brick-full.config
      ];
    "a system built on nixosModules.full follows no repository" =
      let c = (self.lib.mkSystem { modules = own; }).config; in c.mortar.packages.repositories == [ ];
    "the settings that follow it aren't a flake output" =
      !(lib.any (m: m == follows) (lib.attrValues self.nixosModules));
    "a system built on nixosModules.full follows nothing" =
      let u = update own; in !u.enable && u.baseUrl == null;
    "enabling updates with no source doesn't fall back to the public cache" =
      refused (own ++ [ { mortar.update = { enable = true; publicKey = "own-1:AAAA"; }; } ]);
    "a private bucket stays S3 mode" =
      let u = update (own ++ [ { mortar.update = { enable = true; bucket = "b"; endpoint = "https://e"; publicKey = "own-1:AAAA"; configName = "brick-own"; }; } ]);
      in u.enable && u.baseUrl == null;
    "the opt-ins do evaluate for your own system" =
      !(refused [ bare self.nixosModules.pack-ai self.nixosModules.pack-emulation { mortar.play.nonCommercialCores = true; mortar.usb.trustHost = true; } ]);
  };
  failed = lib.attrNames (lib.filterAttrs (_: ok: !ok) checks);
in
assert lib.assertMsg (failed == [ ]) "public-policy: ${lib.concatStringsSep "; " failed}";
runCommand "mortar-public-policy-check" { } ''
  echo PASS > $out
''
