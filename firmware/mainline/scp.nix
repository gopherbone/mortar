# SPDX-License-Identifier: GPL-2.0-or-later
#
# TrimUI Brick SCP (ARISC/OpenRISC) firmware + its service device tree,
# extracted from TrimUI's public stock firmware release, exactly as
# jkingston/trimui-brick-firmware does it (package/brick-scp):
#
#   https://github.com/trimui/firmware_brick/releases/tag/v1.1.1-20251126
#   TG3040_Brick_firmware_v1.1.1.zip
#     -> trimui_tg3040.awimg (IMAGEWTY) -> boot_package.fex (sunxi-package)
#        -> "scp" item (0x14008 bytes) and "dtb" item (0x25600 bytes)
#
# Every step is hash-pinned (zip, boot_package.fex, both items), and the SPL
# checks the two SHA-256s again at boot (U-Boot patch 0004), so a different
# blob would build but not boot.
#
# The blob is proprietary and NOT redistributable: it is fetched at build time
# and must never be committed to git or pushed to a public binary cache.
# meta.license = unfree, so nixpkgs must allow it, e.g.
#   nixpkgs.config.allowUnfreePredicate = pkg: lib.getName pkg == "trimui-brick-scp";
#
# The user's old card holds a byte-identical scp.bin (sha b25eacb0...), but its
# boot_package "dtb" item is a different (Bloom) tree (617081ab...), which the
# SPL would reject, so the card is not a drop-in source for service.dtb.
{
  lib,
  stdenvNoCC,
  fetchurl,
  python3,
}:

stdenvNoCC.mkDerivation {
  pname = "trimui-brick-scp";
  version = "1.1.1-20251126";

  src = fetchurl {
    url = "https://github.com/trimui/firmware_brick/releases/download/v1.1.1-20251126/TG3040_Brick_firmware_v1.1.1.zip";
    # sha256 488db02176e81ad4fdcf9e0bb4dbf406c883199f5b3521445e038bf81c8916ef
    # (brick-scp.hash in jkingston/trimui-brick-firmware)
    hash = "sha256-SI2wIXboGtT9z54LtNv0BsiDGZ9bNSFEXgOL+ByJFu8=";
  };

  nativeBuildInputs = [ python3 ];

  dontUnpack = true;
  dontConfigure = true;
  dontFixup = true;

  buildPhase = ''
    runHook preBuild
    python3 -I ${./extract-scp.py} "$src" out
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    install -Dm0644 out/scp.bin $out/scp.bin
    install -Dm0644 out/service.dtb $out/service.dtb
    runHook postInstall
  '';

  # Not redistributable: keep it off substituters/Hydra.
  preferLocalBuild = true;
  allowSubstitutes = false;

  meta = {
    description = "TrimUI Brick (Allwinner A133) vendor SCP firmware and SCP service DTB";
    homepage = "https://github.com/trimui/firmware_brick";
    license = lib.licenses.unfree;
    sourceProvenance = [ lib.sourceTypes.binaryFirmware ];
    hydraPlatforms = [ ];
    platforms = lib.platforms.all;
  };
}
