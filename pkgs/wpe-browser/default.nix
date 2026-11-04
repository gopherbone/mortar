# SPDX-License-Identifier: GPL-2.0-or-later
#
# WPE Browser by Stanislav Matveyev, https://github.com/matweew/wpe-tsp (MIT, its
# notice is in share/doc/wpe-browser/), written for TrimUI's stock firmware. A web
# browser on WPE WebKit, driven with the pad; here as an app in its own profile
# (app.nix), not part of the system.
#
# How it runs on Mortar. Upstream draws through SDL2 on the panel. Here SDL2 is
# sdl2-compat on mortar-sdl-run's sdl3-pvr, so the same drawing code is a Wayland client
# on the stock EGL through mortar-wlegl, like the other GPU apps. The web process
# renders with the stock EGL, surfaceless, into KMS dumb buffers
# (patches/webkit-dumb-dmabuf-render-targets.patch); the browser process samples those
# as GL textures, draws its menus and keyboard over them and presents. One GPU copy per
# frame. patches/app-mortar.patch has the rest, wpe-browser.sh the environment.
#
# Left out of upstream's package: libcedarc (Allwinner's decoder, no published licence,
# needs a vendor kernel device), so video is software; mpv, yt-dlp and ytp (RustyPipe,
# GPL-3.0), so YouTube plays in the page and nothing tries to update itself.
#
# Licences: the app MIT; WPE WebKit LGPL-2.1 and BSD-style; EasyList and EasyPrivacy
# (share/adblock/rules.json, converted here from a pinned revision) GPL-3.0-or-later or
# CC BY-SA 3.0+. WebKit compiles those rules on first run, into
# ~/.local/share/wpe-browser/content-filters, never into the store.
{
  lib,
  stdenv,
  callPackage,
  fetchurl,
  pkg-config,
  python3,
  bash,
  coreutils,
  dejavu_fonts,
  gawk,
  glib,
  glib-networking,
  gnugrep,
  gst_all_1,
  libGL,
  libsysprof-capture,
  libxkbcommon,
  sdl2-compat,
  SDL2_ttf,
  shared-mime-info,
  xkeyboard_config,
  webkit ? null,
}:

let
  upstream = callPackage ./upstream.nix { };
  wpewebkit = if webkit != null then webkit else callPackage ./webkit.nix { inherit upstream; };

  # What upstream's scripts/package.sh bundles, with PulseAudio (PipeWire's, on Mortar)
  # for ALSA. Only these are on the plugin path. gst-plugins-bad is still in the closure, all
  # of it with its dependencies: WebKit and these plugins link its libraries.
  gstPlugins = stdenv.mkDerivation {
    name = "wpe-browser-gst-plugins";
    dontUnpack = true;
    installPhase = ''
      mkdir -p $out/lib/gstreamer-1.0
      add() { # package, plugins
        local from=$1/lib/gstreamer-1.0; shift
        for p in "$@"; do
          install -m0755 "$from/libgst$p.so" $out/lib/gstreamer-1.0/
        done
      }
      add ${gst_all_1.gstreamer.out} coreelements
      add ${gst_all_1.gst-plugins-base} typefindfunctions playback app gio audioconvert audioresample \
        audiomixer videoconvertscale volume opus vorbis ogg subparse
      add ${gst_all_1.gst-plugins-good} autodetect audioparsers isomp4 matroska id3demux mpg123 wavparse \
        interleave audiofx videofilter deinterlace pulseaudio
      add ${gst_all_1.gst-plugins-bad} opusparse videoparsersbad subenc
      add ${gst_all_1.gst-libav} libav
    '';
    doInstallCheck = true;
    nativeInstallCheckInputs = [ gst_all_1.gstreamer ];
    installCheckPhase = ''
      export HOME=$TMPDIR GST_PLUGIN_SYSTEM_PATH_1_0=$out/lib/gstreamer-1.0 GST_REGISTRY_FORK=no
      export GST_REGISTRY_1_0=$TMPDIR/registry.bin
      for e in avdec_h264 avdec_vp9 avdec_aac qtdemux matroskademux pulsesink playbin3; do
        gst-inspect-1.0 --exists "$e" || { echo "no $e"; gst-inspect-1.0; exit 1; }
      done
    '';
  };

  # Pinned, for a reproducible package: upstream fetches whatever is current when it builds.
  easylistRev = "415e114ca7d6c73a55134495be765ac332503f45";
  easylist = fetchurl {
    url = "https://raw.githubusercontent.com/easylist/easylist/${easylistRev}/easylist.txt";
    hash = "sha256-CXDSdwx/ynKtTA9h//7hDpACa1YuXPfcnrUDlfmq0ns=";
  };
  easyprivacy = fetchurl {
    url = "https://raw.githubusercontent.com/easylist/easylist/${easylistRev}/easyprivacy.txt";
    hash = "sha256-Oe5yDgitr3lePY6bcncy9a+zYXXQQwDS6uBZij+RDdM=";
  };
in
stdenv.mkDerivation {
  pname = "wpe-browser";
  version = "0-unstable-2026-11-01";

  src = upstream;
  patches = [ ./patches/app-mortar.patch ];

  nativeBuildInputs = [ pkg-config python3 ];
  buildInputs = [ glib libGL libsysprof-capture libxkbcommon sdl2-compat SDL2_ttf wpewebkit ];

  buildPhase = ''
    runHook preBuild
    cd app
    $CC -O2 -Wall -Wextra -Wno-unused-parameter -Wno-unused-function \
      -o wpe-tsp browser.c osk.c menu.c downloads.c player.c dmabuf.c ytdlp.c hid.c settings.c favorites.c \
      $(pkg-config --cflags --libs wpe-webkit-2.0 wpe-platform-2.0 sdl2 SDL2_ttf xkbcommon gio-2.0) -lm -ldl
    cd ..
    substituteInPlace scripts/xkb-keymaps.py \
      --replace-fail 'COMPILE = TOOLS + "/usr/libexec/xkbcommon/xkbcli-compile-keymap"' \
                     'COMPILE = "${lib.getOutput "out" libxkbcommon}/libexec/xkbcommon/xkbcli-compile-keymap"' \
      --replace-fail 'ENV = dict(os.environ, LD_LIBRARY_PATH=TOOLS + "/usr/lib/x86_64-linux-gnu")' 'ENV = dict(os.environ)'
    python3 scripts/xkb-keymaps.py ${xkeyboard_config}/share/X11/xkb xkb-keymaps.bin
    python3 scripts/make-adblock.py rules.json ${easylist} ${easyprivacy}
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    install -Dm0755 app/wpe-tsp $out/bin/wpe-tsp

    # The browser finds these next to its binary
    install -Dm0644 xkb-keymaps.bin $out/share/xkb-keymaps.bin
    install -Dm0644 rules.json $out/share/adblock/rules.json
    mkdir -p $out/share/fonts
    for f in DejaVuSans DejaVuSans-Bold DejaVuSerif DejaVuSerif-Bold DejaVuSansMono DejaVuSansMono-Bold; do
      install -m0644 ${dejavu_fonts}/share/fonts/truetype/$f.ttf $out/share/fonts/
    done

    install -Dm0644 app/settings.default.conf $out/share/wpe-browser/settings.default.conf
    mkdir -p $out/share/wpe-browser/xdg/mime
    cp ${shared-mime-info}/share/mime/mime.cache $out/share/wpe-browser/xdg/mime/
    cat > $out/share/wpe-browser/fonts.conf <<EOF
    <?xml version="1.0"?>
    <fontconfig>
      <dir>$out/share/fonts</dir>
      <dir prefix="xdg">fonts</dir>
      <cachedir prefix="xdg">fontconfig</cachedir>
      <alias><family>sans-serif</family><prefer><family>DejaVu Sans</family></prefer></alias>
      <alias><family>serif</family><prefer><family>DejaVu Serif</family></prefer></alias>
      <alias><family>monospace</family><prefer><family>DejaVu Sans Mono</family></prefer></alias>
    </fontconfig>
    EOF

    install -Dm0644 LICENSE $out/share/doc/wpe-browser/LICENSE
    install -Dm0644 licenses/README.md $out/share/doc/wpe-browser/upstream-licenses.md

    substitute ${./wpe-browser.sh} $out/bin/wpe-browser \
      --subst-var-by bash ${bash}/bin/bash \
      --subst-var-by coreutils ${coreutils} \
      --subst-var-by gnugrep ${gnugrep} \
      --subst-var-by gawk ${gawk} \
      --subst-var-by out $out \
      --subst-var-by glibNetworking ${glib-networking} \
      --subst-var-by xkeyboardConfig ${xkeyboard_config} \
      --subst-var-by gstPlugins ${gstPlugins}
    chmod +x $out/bin/wpe-browser
    runHook postInstall
  '';

  passthru = { inherit gstPlugins upstream; webkit = wpewebkit; };

  meta = {
    description = "WPE Browser: a web browser for the pad, by Stanislav Matveyev";
    homepage = "https://github.com/matweew/wpe-tsp";
    license = lib.licenses.mit;
    mainProgram = "wpe-browser";
    platforms = [ "aarch64-linux" ];
  };
}
