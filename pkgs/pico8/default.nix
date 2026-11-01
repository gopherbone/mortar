# SPDX-License-Identifier: GPL-2.0-or-later
# PICO-8 (Lexaloffle) from the user's own licensed Raspberry Pi zip.
#
# PICO-8 is commercial: the zip is NEVER committed or fetched. Add your copy
# to the local Nix store once, on the machine that evaluates the flake:
#
#   nix-store --add-fixed sha256 pico-8_0.2.7_raspi.zip
#
# It is not part of the system closure (CI never sees the zip, the update
# cache never holds PICO-8): `mortar pico8` builds this on your computer
# (flake output packages.aarch64-linux.pico8) and installs it into its own
# profile on the Brick, /nix/var/nix/profiles/pico8 (modules/mortar-apps.nix).
#
# Uses the 64-bit build (pico8_64: aarch64, needs only libSDL2 + libc),
# linked against nixpkgs' SDL2 (sdl2-compat on SDL3), so it gets the same
# display paths as Mortar's SDL3 apps (mortar-sdl-run: GPU or software GL;
# Wayland client inside mortar-shell). Unfree: never push the closure to a
# public cache.
{ lib, stdenv, requireFile, unzip, autoPatchelfHook, makeWrapper, SDL2, wget }:

stdenv.mkDerivation {
  pname = "pico-8";
  version = "0.2.7";

  src = requireFile {
    name = "pico-8_0.2.7_raspi.zip";
    sha256 = "5e64187b50c470b2e345ea0ccccb04a9c098660cc8b9974672f6587741893295";
    message = ''
      PICO-8 is commercial software, so Mortar does not download it.
      Buy it from https://www.lexaloffle.com/pico-8.php, download the
      Raspberry Pi zip (pico-8_0.2.7_raspi.zip), and add it to the Nix store:

        nix-store --add-fixed sha256 pico-8_0.2.7_raspi.zip
    '';
  };

  nativeBuildInputs = [ unzip autoPatchelfHook makeWrapper ];
  buildInputs = [ SDL2 ];
  dontStrip = true;

  unpackPhase = ''
    runHook preUnpack
    unzip -q $src
    runHook postUnpack
  '';

  # pico8_64 looks for pico8.dat next to its own (resolved) path.
  installPhase = ''
    runHook preInstall
    install -Dm0755 pico-8/pico8_64 $out/lib/pico-8/pico8_64
    install -Dm0644 pico-8/pico8.dat $out/lib/pico-8/pico8.dat
    install -Dm0644 pico-8/lexaloffle-pico8.png $out/share/pico-8/icon.png
    install -Dm0644 pico-8/pico-8_manual.txt $out/share/doc/pico-8/manual.txt
    install -Dm0644 pico-8/license.txt $out/share/doc/pico-8/license.txt
    # Splore/BBS downloads shell out to `wget` (codo_wget runs it via sh).
    makeWrapper $out/lib/pico-8/pico8_64 $out/bin/pico8 \
      --prefix PATH : ${lib.makeBinPath [ wget ]}
    runHook postInstall
  '';

  meta = {
    description = "PICO-8 fantasy console (user-supplied licensed copy)";
    homepage = "https://www.lexaloffle.com/pico-8.php";
    license = lib.licenses.unfree;
    platforms = [ "aarch64-linux" ];
    mainProgram = "pico8";
  };
}
