# SPDX-License-Identifier: GPL-2.0-or-later
# WPE WebKit for the Brick's browser, the WPEPlatform API only (no libwpe, no
# Wayland or DRM platform: the app brings its own WPEPlatform, see default.nix),
# with the options and patches of upstream wpe-tsp (scripts/build-webkit.sh).
# nixpkgs has no wpewebkit, so nothing here comes from a cache: about half an hour on a
# Mac's 8-core aarch64 builder at 5 jobs. Same compiler as nixpkgs' webkitgtk 2.54.1.
#
# WebKit keeps its own licences (LGPL-2.1, BSD-style; ANGLE, Skia, ... BSD/MIT/Apache).
#
# From upstream's patches/, all that apply to a mainline kernel and a modern compiler:
#   0001 surfaceless EGL on a driver without client extensions (the stock PowerVR)
#   0005 builds without libdrm
#   0006, 0010 core GLES functions through dlsym: the stock eglGetProcAddress hides them
#   0007 WEBKIT_DISABLE_PSON, WEBKIT_DISABLE_WEB_PROCESS_CACHE: one web process
#   0012 YUV video frames go to the GPU as planes
#   0013 decoder threads (it also names upstream's cedar decoder, which isn't here)
# Left out: 0002, 0009, 0011 work around GCC 12, 0004 Debian's multiarch. 0008 (ION) is
# replaced by patches/webkit-dumb-dmabuf-render-targets.patch: no /dev/ion on mainline.
{
  lib,
  clangStdenv,
  fetchurl,
  upstream,
  bison,
  cmake,
  flex,
  gettext,
  gperf,
  ninja,
  perl,
  pkg-config,
  python3,
  ruby,
  unifdef,
  glib,
  libsoup_3,
  gst_all_1,
  libepoxy,
  libGL,
  libxkbcommon,
  freetype,
  fontconfig,
  harfbuzzFull,
  icu,
  lcms2,
  libgcrypt,
  libgpg-error,
  libjpeg,
  libpng,
  libtasn1,
  libwebp,
  libxml2,
  libxslt,
  sqlite,
  woff2,
  zlib,
  buildJobs ? 5,
}:

clangStdenv.mkDerivation (finalAttrs: {
  pname = "wpewebkit";
  version = "2.54.1";

  src = fetchurl {
    url = "https://wpewebkit.org/releases/wpewebkit-${finalAttrs.version}.tar.xz";
    hash = "sha256-Dei9+6AR+c1jupqRcfpgON0sCCN4lPQIKru4IfKdP+k=";
  };

  patches = map (p: "${upstream}/patches/${p}.patch") [
    "0001-surfaceless-default-display-fallback"
    "0005-drm-fourcc-fallback-without-libdrm"
    "0006-skia-epoxy-core-gles3-proc-fallback"
    "0007-env-single-web-process"
  ] ++ [
    ./patches/webkit-dumb-dmabuf-render-targets.patch
  ] ++ map (p: "${upstream}/patches/${p}.patch") [
    "0010-angle-gles-proc-dlsym-fallback"
    "0012-yuv-video-frames-uploaded-to-gpu"
    "0013-decoder-settings-for-a133"
  ];

  postPatch = ''
    patchShebangs .
  '';

  outputs = [ "out" "dev" ];

  nativeBuildInputs = [
    bison
    cmake
    flex
    gettext
    gperf
    ninja
    perl
    perl.pkgs.FileCopyRecursive
    pkg-config
    python3
    ruby
    unifdef
    glib # gdbus-codegen
  ];

  buildInputs = [
    freetype
    fontconfig
    gst_all_1.gst-plugins-bad
    gst_all_1.gst-plugins-base
    harfbuzzFull
    icu
    lcms2
    libepoxy
    libGL
    libgcrypt
    libgpg-error
    libjpeg
    libpng
    libtasn1
    libwebp
    libxkbcommon
    libxml2
    libxslt
    sqlite
    woff2
    zlib
  ];

  propagatedBuildInputs = [ glib libsoup_3 ];

  cmakeFlags = [
    "-DPORT=WPE"
    "-DENABLE_WPE_PLATFORM=ON"
    "-DENABLE_WPE_PLATFORM_HEADLESS=ON"
    "-DENABLE_WPE_PLATFORM_DRM=OFF"
    "-DENABLE_WPE_PLATFORM_WAYLAND=OFF"
    "-DENABLE_WPE_LEGACY_API=OFF"
    "-DENABLE_WPE_QT_API=OFF"
    "-DUSE_GBM=OFF"
    "-DUSE_LIBDRM=OFF"
    "-DENABLE_GPU_PROCESS=OFF"
    "-DENABLE_DOCUMENTATION=OFF"
    "-DENABLE_INTROSPECTION=OFF"
    "-DENABLE_JOURNALD_LOG=OFF"
    "-DENABLE_BUBBLEWRAP_SANDBOX=OFF"
    "-DENABLE_WEBDRIVER=OFF"
    "-DENABLE_SPELLCHECK=OFF"
    "-DENABLE_SPEECH_SYNTHESIS=OFF"
    "-DUSE_GSTREAMER=ON"
    "-DUSE_GSTREAMER_GL=OFF"
    "-DUSE_GSTREAMER_WEBRTC=OFF"
    "-DENABLE_VIDEO=ON"
    "-DENABLE_WEB_AUDIO=ON"
    "-DENABLE_MEDIA_SOURCE=ON"
    "-DENABLE_VIDEO_USES_ELEMENT_FULLSCREEN=ON"
    "-DENABLE_MEDIA_CONTROLS_CONTEXT_MENUS=ON"
    "-DENABLE_MEDIA_STREAM=OFF"
    "-DENABLE_MEDIA_RECORDER=OFF"
    "-DENABLE_MEDIA_SESSION=OFF"
    "-DENABLE_WEB_CODECS=OFF"
    "-DENABLE_ENCRYPTED_MEDIA=OFF"
    "-DENABLE_GAMEPAD=OFF"
    "-DENABLE_WEBGL=ON"
    "-DENABLE_MINIBROWSER=OFF"
    "-DENABLE_API_TESTS=OFF"
    "-DUSE_ATK=OFF"
    "-DUSE_AVIF=OFF"
    "-DUSE_JPEGXL=OFF"
    "-DUSE_LIBHYPHEN=OFF"
    "-DUSE_LIBBACKTRACE=OFF"
    "-DUSE_SYSPROF_CAPTURE=OFF"
  ];

  # (spaces: cmakeFlags would split these)
  preConfigure = ''
    cmakeFlagsArray+=(
      "-DCMAKE_C_FLAGS_RELEASE=-O2 -DNDEBUG -g0"
      "-DCMAKE_CXX_FLAGS_RELEASE=-O2 -DNDEBUG -g0"
    )
  '';

  # The builder is 16 GB without swap and shared; a WebCore job takes about 2 GB.
  preBuild = ''
    export NIX_BUILD_CORES=${toString buildJobs}
  '';

  requiredSystemFeatures = [ "big-parallel" ];

  meta = {
    description = "WPE WebKit: the web engine of the Brick's browser";
    homepage = "https://wpewebkit.org/";
    license = with lib.licenses; [ lgpl21Plus bsd2 ];
    platforms = [ "aarch64-linux" ];
  };
})
