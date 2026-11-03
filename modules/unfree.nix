# SPDX-License-Identifier: GPL-2.0-or-later
# Which unfree packages this system may use. Device, pack and port modules each
# add theirs; nothing else may set nixpkgs.config.allowUnfreePredicate.
#
# These names are the line between what a machine builds for itself and what
# may be cached or published: vendor blobs, non-commercial cores and commercial
# software stay out of anything public (docs/updates.md).
{ config, lib, ... }:

{
  options.mortar.unfree = lib.mkOption {
    type = lib.types.listOf lib.types.str;
    default = [ ];
    example = [ "pico-8" ];
    description = "Package names (lib.getName) allowed despite an unfree licence.";
  };

  config.nixpkgs.config.allowUnfreePredicate = pkg: builtins.elem (lib.getName pkg) config.mortar.unfree;
}
