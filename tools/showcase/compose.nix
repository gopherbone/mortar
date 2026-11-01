# SPDX-License-Identifier: GPL-2.0-or-later
# `nix run .#showcase -- --capture DIR --cut rough|full --layout wide|square --out FILE.mp4`
# (macOS or Linux; DIR is the showcase-capture output). See README.md.
{ writeShellApplication, python3, ffmpeg }:

writeShellApplication {
  name = "mortar-showcase";
  runtimeInputs = [ (python3.withPackages (ps: [ ps.pillow ps.numpy ])) ffmpeg ];
  text = ''
    exec python3 ${./compose.py} "$@"
  '';
}
