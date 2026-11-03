# SPDX-License-Identifier: GPL-2.0-or-later
# What may be published (docs/updates.md, modules/unfree.nix), at evaluation:
#
#   - bare and full evaluate to the same system with mortar.public on, which
#     refuses every unfree package outside mortar.publicUnfree. So nothing else
#     unfree is in them, however it got there.
#   - The opt-ins are in fact refused: Claude Code, the non-commercial cores, and
#     a root console for USB hosts.
#
# `lib.evalMatrix` covers the rest; CI's closure check is the second line, by name,
# for what isn't marked unfree.
{ lib, runCommand, self }:

let
  drv = modules: (self.lib.mkSystem { inherit modules; }).config.system.build.toplevel.drvPath;
  public = { mortar.public = true; };
  bare = ../configurations/bare.nix;
  full = ../configurations/full.nix;
  refused = m: !(builtins.tryEval (builtins.deepSeq (drv m) true)).success;

  checks = {
    "bare holds only what may be published" = drv [ bare public ] == self.nixosConfigurations.brick-bare.config.system.build.toplevel.drvPath;
    "full holds only what may be published" = drv [ full public ] == self.nixosConfigurations.brick-full.config.system.build.toplevel.drvPath;
    "Claude Code is refused" = refused [ bare self.nixosModules.pack-ai public ];
    "non-commercial cores are refused" =
      refused [ bare self.nixosModules.pack-emulation { mortar.play.nonCommercialCores = true; } public ];
    "a trusted USB host is refused" = refused [ bare { mortar.usb.trustHost = true; } public ];
    "the opt-ins do evaluate for your own system" =
      !(refused [ bare self.nixosModules.pack-ai self.nixosModules.pack-emulation { mortar.play.nonCommercialCores = true; mortar.usb.trustHost = true; } ]);
  };
  failed = lib.attrNames (lib.filterAttrs (_: ok: !ok) checks);
in
assert lib.assertMsg (failed == [ ]) "public-policy: ${lib.concatStringsSep "; " failed}";
runCommand "mortar-public-policy-check" { } ''
  echo PASS > $out
''
