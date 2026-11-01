# SPDX-License-Identifier: GPL-2.0-or-later
# mortar-wlegl: Wayland EGL window surfaces on the stock PowerVR EGL, for
# apps inside mortar-shell. See the header of mortar-wlegl.c.
#
#   lib/libmortar-wlegl.so   use as libEGL and libGLESv2 (SDL_EGL_LIBRARY,
#                            SDL_OPENGL_LIBRARY); everything it does not
#                            wrap resolves to the stock libs it links
#   bin/mortar-wlegl-demo    GLES test client (fps + CPU, orientation marker)
{ lib, stdenv, pkg-config, wayland, wayland-scanner, wayland-protocols, libdrm, libglvnd, pvr-ddk }:

stdenv.mkDerivation {
  pname = "mortar-wlegl";
  version = "0";
  src = lib.fileset.toSource {
    root = ./.;
    fileset = lib.fileset.unions [ ./mortar-wlegl.c ./wlegl-demo.c ];
  };
  nativeBuildInputs = [ pkg-config wayland-scanner ];
  buildInputs = [ wayland libdrm ];
  buildPhase = ''
    runHook preBuild
    P=${wayland-protocols}/share/wayland-protocols
    wayland-scanner client-header $P/unstable/linux-dmabuf/linux-dmabuf-unstable-v1.xml linux-dmabuf-unstable-v1-client-protocol.h
    wayland-scanner private-code $P/unstable/linux-dmabuf/linux-dmabuf-unstable-v1.xml linux-dmabuf-unstable-v1-protocol.c
    wayland-scanner client-header $P/stable/xdg-shell/xdg-shell.xml xdg-shell-client-protocol.h
    wayland-scanner private-code $P/stable/xdg-shell/xdg-shell.xml xdg-shell-protocol.c
    I="-I. -I${libglvnd.dev}/include $(pkg-config --cflags wayland-client wayland-egl-backend libdrm)"
    PVR="-L${pvr-ddk}/lib -Wl,-rpath,${pvr-ddk}/lib -lEGL -lGLESv2"
    $CC -O2 -Wall -Wextra -Wno-unused-parameter -Werror=implicit-function-declaration -fPIC -shared \
      -Wl,-soname,libmortar-wlegl.so -Wl,-z,defs \
      -DPVR_LIBDIR='"${pvr-ddk}/lib"' $I \
      -o libmortar-wlegl.so mortar-wlegl.c linux-dmabuf-unstable-v1-protocol.c \
      $PVR $(pkg-config --libs wayland-client libdrm) -ldl -lpthread
    # our lib first, so its egl*/gl* wrappers win over the stock ones
    $CC -O2 -Wall -Wextra -Wno-unused-parameter $I $(pkg-config --cflags wayland-egl) \
      -o mortar-wlegl-demo wlegl-demo.c xdg-shell-protocol.c \
      -L. -Wl,-rpath,$out/lib -lmortar-wlegl $PVR $(pkg-config --libs wayland-client wayland-egl) -lm
    runHook postBuild
  '';
  installPhase = ''
    runHook preInstall
    install -Dm0755 libmortar-wlegl.so $out/lib/libmortar-wlegl.so
    install -Dm0755 mortar-wlegl-demo $out/bin/mortar-wlegl-demo
    runHook postInstall
  '';
  # the stock libs are unfree and aarch64-only
  meta = {
    description = "Wayland EGL platform for the stock PowerVR GE8300 EGL (TrimUI Brick)";
    license = lib.licenses.gpl2Plus;
    platforms = [ "aarch64-linux" ];
  };
}
