# SPDX-License-Identifier: GPL-2.0-or-later
#
# SDL3 that can put GLES on the panel through the PowerVR GPU.
#
# The stock DDK userspace has only a NULL WSEGL: EGL window surfaces take
# EGL_DEFAULT_DISPLAY and a NULL native window (a `struct fbdev_window`
# pointer is refused with EGL_BAD_NATIVE_WINDOW on Mortar) and present
# full-screen through our DC driver (brick_dc_drm). SDL2's "mali" backend did exactly
# that; SDL3 dropped it, but its "vivante" backend has the same shape (an
# fbdev-style native display/window from fb* calls, EGL, evdev input). So:
#   - build nixpkgs' SDL3 with the vivante backend (normally ARM32 only),
#     non-VDK flavour, against a stub <EGL/eglvivante.h>;
#   - load the fb* calls from libpvr-fbwin.so (fbwin.c) instead of the
#     Vivante libEGL, accept its NULL native window, and default GL
#     requests to GLES 2 (vivante-pvr.patch);
#   - don't link libEGL: SDL loads the IMG EGL/GLES at run time.
#
# Same soname and ABI as nixpkgs' SDL3, so existing SDL3 apps use it via
# LD_LIBRARY_PATH (see passthru.env / mortar.gpu.sdlEnv).
#
# Inside mortar-shell (a Wayland compositor owns the display, so the DC path
# is off limits) apps use SDL's Wayland backend instead, on the same stock
# EGL through mortar-wlegl (pkgs/mortar-wlegl): GPU-rendered dma-bufs the
# shell scans out directly. wayland-pvr.patch lets the GLES2 renderer draw
# the window top-down there (no flip, no copy) and defaults GL to ES 2.0.
# passthru.envWayland is that environment.
{ lib, stdenv, sdl3, pvr-ddk, mortar-wlegl, writeTextDir }:

let
  fbwin = stdenv.mkDerivation {
    pname = "pvr-fbwin";
    version = "0";
    src = ./fbwin.c;
    dontUnpack = true;
    buildPhase = "$CC -O2 -Wall -shared -fPIC -o libpvr-fbwin.so $src";
    installPhase = "install -Dm0755 libpvr-fbwin.so $out/lib/libpvr-fbwin.so";
  };

  # The configure check only needs the header to exist; SDL uses its own
  # Khronos headers.
  vivanteHeader = writeTextDir "include/EGL/eglvivante.h" ''
    #include <EGL/egl.h>
  '';

  sdl = sdl3.overrideAttrs (old: {
    pname = "sdl3-pvr";
    # fb* calls from libpvr-fbwin.so; NULL native window; GLES 2 by default.
    # Wayland on mortar-wlegl: top-down window in the GLES2 renderer.
    patches = (old.patches or [ ]) ++ [ ./vivante-pvr.patch ./wayland-pvr.patch ];
    postPatch = (old.postPatch or "") + ''
      substituteInPlace CMakeLists.txt --replace-fail \
        '"Use Vivante EGL video driver" ON "''${UNIX_SYS};SDL_CPU_ARM32" OFF' \
        '"Use Vivante EGL video driver" ON "''${UNIX_SYS}" OFF'
      substituteInPlace cmake/sdlchecks.cmake --replace-fail \
        'sdl_link_dependency(vivante LIBS EGL)' ""
      substituteInPlace src/video/vivante/SDL_vivantevideo.c \
        --replace-fail '@fbwin@' '${fbwin}/lib/libpvr-fbwin.so'
    '';
    cmakeFlags = (old.cmakeFlags or [ ]) ++ [
      "-DSDL_VIVANTE=ON"
      "-DCMAKE_REQUIRED_INCLUDES=${vivanteHeader}/include"
    ];
    env = (old.env or { }) // {
      # EGLNativeWindowType is an integer in SDL's generic-unix Khronos
      # headers and a pointer in Vivante's; the backend mixes the two.
      NIX_CFLAGS_COMPILE = toString [
        (old.env.NIX_CFLAGS_COMPILE or "")
        "-I${vivanteHeader}/include"
        "-Wno-error=int-conversion"
      ];
    };
    # nixpkgs' SDL3 runs its test suite; the vivante backend adds nothing to it
    doCheck = false;
    passthru = (old.passthru or { }) // {
      inherit fbwin mortar-wlegl;
      # Run an SDL3 app on the GPU: SDL_VIDEO_DRIVER=vivante, IMG EGL/GLES.
      env = {
        LD_LIBRARY_PATH = "${lib.getLib sdl}/lib";
        SDL_VIDEO_DRIVER = "vivante";
        SDL_EGL_LIBRARY = "${pvr-ddk}/lib/libEGL.so.1";
        SDL_OPENGL_LIBRARY = "${pvr-ddk}/lib/libGLESv2.so";
        SDL_RENDER_DRIVER = "opengles2";
      };
      # Run an SDL3 app on the GPU as a Wayland client (inside mortar-shell).
      envWayland = {
        LD_LIBRARY_PATH = "${lib.getLib sdl}/lib";
        SDL_VIDEO_DRIVER = "wayland";
        SDL_EGL_LIBRARY = "${mortar-wlegl}/lib/libmortar-wlegl.so";
        SDL_OPENGL_LIBRARY = "${mortar-wlegl}/lib/libmortar-wlegl.so";
        SDL_RENDER_DRIVER = "opengles2";
        SDL_MORTAR_GL_DEFAULT_PROFILE = "es2";
      };
    };
  });
in
sdl
