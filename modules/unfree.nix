# SPDX-License-Identifier: GPL-2.0-or-later
# Which unfree packages this system may use. Device, pack and port modules each
# add theirs; nothing else may set nixpkgs.config.allowUnfreePredicate.
#
# What may be published is narrower. TrimUI's files for the Brick are hosted in the
# public bare image and cache: the project's decision, because TrimUI publishes
# them (docs/updates.md). It is not a licence: they stay proprietary, and nobody
# here can grant rights in them. Three emulator cores have non-commercial licences that
# allow free distribution, so they are published too, which is why Mortar can't be
# sold or take donations (LICENSE). Commercial software (Claude Code) is never in
# anything public.
{ config, lib, ... }:

let
  cfg = config.mortar;
in
{
  options.mortar = {
    unfree = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      default = [ ];
      example = [ "pico-8" ];
      description = "Package names (lib.getName) allowed despite an unfree licence.";
    };

    publicUnfree = lib.mkOption {
      type = lib.types.listOf lib.types.str;
      readOnly = true;
      default = [ "trimui-brick-scp" "u-boot-trimui-brick-mainline" "trimui-brick-firmware" "pvr-ddk-userspace" "libretro-snes9x2005-plus" "libretro-genesis-plus-gx" "libretro-fbneo" ];
      description = "The unfree packages the project has decided may be in a public image or cache.";
    };

    public = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = ''
        Evaluate as a system to publish: every unfree package outside
        `mortar.publicUnfree` is refused, whatever `mortar.unfree` allows, and
        `mortar.usb.trustHost` is refused. Set by CI's checks (tests/public-policy.nix).
      '';
    };
  };

  config = {
    nixpkgs.config.allowUnfreePredicate =
      pkg: builtins.elem (lib.getName pkg) (if cfg.public then cfg.publicUnfree else cfg.unfree);

    assertions = [
      {
        assertion = !(cfg.public && cfg.usb.trustHost or false);
        message = "mortar.public: a published system must not trust USB hosts (mortar.usb.trustHost).";
      }
    ];
  };
}
