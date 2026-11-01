# SPDX-License-Identifier: GPL-2.0-or-later
# The three type families from the Mortar UI mockups, all SIL OFL 1.1:
#   Bricolage Grotesque (display)  google/fonts at a fixed commit; it only
#                                  ships as a variable font, so the weights
#                                  and optical sizes the UI uses are cut into
#                                  static instances here (fontdue can't pick
#                                  variable-font instances at runtime).
#   IBM Plex Sans (body)           nixpkgs ibm-plex, sans family only
#   JetBrains Mono (mono)          nixpkgs jetbrains-mono
{
  lib,
  runCommand,
  fetchurl,
  python3Packages,
  ibm-plex,
  jetbrains-mono,
}:

let
  rev = "6ce172f74aa355ea43eb964fa4a91570a4d3064d";
  base = "https://raw.githubusercontent.com/google/fonts/${rev}/ofl/bricolagegrotesque";
  bricolage = fetchurl {
    name = "BricolageGrotesque-VF.ttf";
    url = "${base}/BricolageGrotesque%5Bopsz,wdth,wght%5D.ttf";
    hash = "sha256-QT5zV4Cd3RL9gKlqijlt4OQBY41KzTyz43Uy8EcqxoI=";
  };
  bricolageLicense = fetchurl {
    name = "BricolageGrotesque-OFL.txt";
    url = "${base}/OFL.txt";
    hash = "sha256-S1p9jzf1YCYhyKjXNYpqLnExfmwjHGYeFa7wJ10+B7o=";
  };
  plex = ibm-plex.override { families = [ "sans" ]; };
in
runCommand "mortar-ui-fonts"
  {
    nativeBuildInputs = [ python3Packages.fonttools ];
    meta = {
      description = "Fonts for the Mortar on-device UI";
      license = lib.licenses.ofl;
      platforms = lib.platforms.all;
    };
  }
  ''
    d=$out/share/fonts/mortar-ui
    mkdir -p $d $out/share/licenses/mortar-ui-fonts

    # name     weight  optical size (CSS opsz follows the px size it's used at)
    cut() { fonttools varLib.instancer -q -o "$d/BricolageGrotesque-$1.ttf" ${bricolage} wght=$2 opsz=$3 wdth=100; }
    cut ExtraBold 800 64   # screen titles (d-h, ~65 px)
    cut Bold      700 42   # section titles (d-h2, ~43 px), tile names
    cut ExtraBoldSmall 800 28  # profile initials (~31 px)

    cp ${plex}/share/fonts/truetype/IBMPlexSans-{Regular,Medium,SemiBold}.ttf $d/
    cp ${jetbrains-mono}/share/fonts/truetype/JetBrainsMono-{Regular,SemiBold}.ttf $d/

    cp ${bricolageLicense} $out/share/licenses/mortar-ui-fonts/BricolageGrotesque-OFL.txt
  ''
