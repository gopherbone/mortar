# SPDX-License-Identifier: GPL-2.0-or-later
# The three type families of the Mortar UI style (v2, after the "Pond"
# visual language; see src/theme.rs), all SIL OFL 1.1, from google/fonts at
# a fixed commit:
#   Instrument Serif (display)  static Regular + Italic: titles, clock, names
#   Onest (UI)                  variable; cut into static 400/500/600
#   Geist Mono (labels, hints)  variable; cut into static 400/600
# fontdue can't pick variable-font instances at runtime, so the weights the
# UI uses are instanced here with fonttools.
{
  lib,
  runCommand,
  fetchurl,
  python3Packages,
}:

let
  rev = "6c1dbe27dcf7d817d4b62cec033ba77fa8e2a5ec";
  get = path: name: hash: fetchurl { inherit name hash; url = "https://raw.githubusercontent.com/google/fonts/${rev}/ofl/${path}"; };
  serifRegular = get "instrumentserif/InstrumentSerif-Regular.ttf" "InstrumentSerif-Regular.ttf" "sha256-SY79Rh9t38t6ERv5pWVwnSCF1IIB1QHq2WDZPoT/u4g=";
  serifItalic = get "instrumentserif/InstrumentSerif-Italic.ttf" "InstrumentSerif-Italic.ttf" "sha256-CJObi99TSv7CSuDvXgP5SJQM2aj+COf+y60EDmIyc4U=";
  serifLicense = get "instrumentserif/OFL.txt" "InstrumentSerif-OFL.txt" "sha256-Ep7XYYlZcWlZ8pQf3VtJ4K1ubB14cmdheGoAJT2GVSE=";
  onest = get "onest/Onest%5Bwght%5D.ttf" "Onest-VF.ttf" "sha256-lmxcKbR1XahLaFTVwh3U6qJCAiXQ6YdN5gLeF21KnzE=";
  onestLicense = get "onest/OFL.txt" "Onest-OFL.txt" "sha256-BxGV2IBuIm+u7mAlnCjKZ7RYInr1GVpz9c/KsG4wA7w=";
  geistMono = get "geistmono/GeistMono%5Bwght%5D.ttf" "GeistMono-VF.ttf" "sha256-0A5ZC46zpZrMMpstBE/RQ66TUJC32jMZnr7ifMfegZY=";
  geistMonoLicense = get "geistmono/OFL.txt" "GeistMono-OFL.txt" "sha256-F4HSgGoH2RxO30dAuIRJ+rfQ6trVP3w1G5TNTU64wA8=";
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
    l=$out/share/licenses/mortar-ui-fonts
    mkdir -p $d $l

    cp ${serifRegular} $d/InstrumentSerif-Regular.ttf
    cp ${serifItalic} $d/InstrumentSerif-Italic.ttf
    for w in 400:Regular 500:Medium 600:SemiBold; do
      fonttools varLib.instancer -q -o "$d/Onest-''${w#*:}.ttf" ${onest} wght=''${w%%:*}
    done
    for w in 400:Regular 600:SemiBold; do
      fonttools varLib.instancer -q -o "$d/GeistMono-''${w#*:}.ttf" ${geistMono} wght=''${w%%:*}
    done

    cp ${serifLicense} $l/InstrumentSerif-OFL.txt
    cp ${onestLicense} $l/Onest-OFL.txt
    cp ${geistMonoLicense} $l/GeistMono-OFL.txt
  ''
