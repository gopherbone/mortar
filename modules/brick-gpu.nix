# SPDX-License-Identifier: GPL-2.0-or-later
# PowerVR GE8300 on the Brick: stock DDK 1.19@6345021 userspace + a display
# class (DC) driver so EGL window surfaces scan out to the panel.
#
#   pvrsrvkm        GPU kernel driver (in jkingston's kernel)
#   brick_dc_drm    DC driver: an in-kernel DRM client of sun4i-drm that owns
#                   three scanout buffers and flips them with blocking atomic
#                   commits (vsync-paced). The stock libs refuse to
#                   initialise EGL without a DC.
#   pvr-ddk         stock userspace (EGL/GLES), autopatchelf'd; NULL WSEGL
#                   only, so apps pass a `struct fbdev_window` (SDL "mali"
#                   backend, RetroArch mali_fbdev) or render into dma-bufs.
#
# SDL3 apps: `mortar-sdl-run APP...` runs an SDL3 app on the GPU through
# pkgs/sdl3-pvr (SDL's vivante backend on the NULL WSEGL, GLES2 renderer,
# zero-copy DC flips) and falls back to SDL's KMSDRM backend on Mesa's
# software GL if the app fails to start that way. MORTAR_SDL=software
# skips the GPU attempt. Only one GL app can own the DC at a time.
#
# Inside mortar-shell the shell owns the display, so the DC path is off
# limits; the shell sets MORTAR_SDL=wayland-gpu and apps run as Wayland
# clients on the same stock EGL through mortar-wlegl (pkgs/mortar-wlegl):
# GPU-rendered dma-bufs that the shell scans out directly. Any number of
# such apps can run at once. Same fallback to software GL.
#
# Notes: a GPU fault can latch the GPU until reboot. A GL window app replaces
# the console on screen while it runs; the console comes back when it exits.
{ config, lib, pkgs, ... }:

let
  cfg = config.mortar.gpu;
  kp = config.boot.kernelPackages;
in
{
  options.mortar.gpu = {
    enable = lib.mkEnableOption "PowerVR GPU support (stock userspace + DC driver)" // { default = true; };
    trace = lib.mkOption {
      type = lib.types.bool;
      default = false;
      description = "Log every DC call from brick_dc_drm (bring-up aid).";
    };
    package = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/pvr-ddk-userspace.nix {
        trimui-brick-firmware = pkgs.callPackage ../pkgs/trimui-brick-firmware.nix { };
      };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/pvr-ddk-userspace.nix { ... }";
      description = "Stock PowerVR userspace libraries.";
    };
    wlegl = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/mortar-wlegl { pvr-ddk = cfg.package; };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/mortar-wlegl { pvr-ddk = config.mortar.gpu.package; }";
      description = "Wayland EGL platform for the stock EGL (GPU apps inside mortar-shell).";
    };
    sdl3 = lib.mkOption {
      type = lib.types.package;
      default = pkgs.callPackage ../pkgs/sdl3-pvr { pvr-ddk = cfg.package; mortar-wlegl = cfg.wlegl; };
      defaultText = lib.literalExpression "pkgs.callPackage ../pkgs/sdl3-pvr { pvr-ddk = config.mortar.gpu.package; mortar-wlegl = config.mortar.gpu.wlegl; }";
      description = "SDL3 with the PowerVR (vivante-style) video backend.";
    };
    sdlRun = lib.mkOption {
      type = lib.types.package;
      readOnly = true;
      description = "`mortar-sdl-run`: start an SDL3 app on the GPU, falling back to software GL.";
    };
  };

  config = lib.mkMerge [ {
    mortar.gpu.sdlRun = pkgs.writeShellApplication {
      name = "mortar-sdl-run";
      runtimeInputs = [ pkgs.coreutils ];
      text = ''
        # usage: mortar-sdl-run APP [ARGS...]
        # MORTAR_SDL: unset = GPU on the panel (DC), wayland-gpu = GPU as a
        # Wayland client (mortar-shell), software = Mesa llvmpipe only.
        if [ "''${MORTAR_SDL:-}" = software ] || ${if cfg.enable then "false" else "true"}; then
          exec "$@"
        fi
        # An app that cannot get a GPU window exits within a second or two
        # (e.g. the GPU is latched after a fault, or another GL app holds
        # the DC). Then run it again on software GL.
        start=$(date +%s)
        status=0
        if [ "''${MORTAR_SDL:-}" = wayland-gpu ]; then
          ${lib.concatStringsSep " " (lib.mapAttrsToList (k: v: "${k}=${lib.escapeShellArg v}") cfg.sdl3.envWayland)} \
            "$@" || status=$?
        else
          ${lib.concatStringsSep " " (lib.mapAttrsToList (k: v: "${k}=${lib.escapeShellArg v}") cfg.sdl3.env)} \
            "$@" || status=$?
        fi
        if [ "$status" -ne 0 ] && [ $(( $(date +%s) - start )) -lt 5 ]; then
          echo "mortar-sdl-run: GPU start failed ($status), using software GL" >&2
          exec "$@"
        fi
        exit "$status"
      '';
    };
  } (lib.mkIf cfg.enable {
    boot.extraModulePackages = [
      # must match the kernel's compiler (GCC 15, see trimui-brick.nix)
      (kp.callPackage ../pkgs/brick-dc-drm { stdenv = pkgs.gcc15Stdenv; })
    ];
    boot.kernelModules = [ "pvrsrvkm" "brick_dc_drm" ];
    boot.extraModprobeConfig = ''
      options brick_dc_drm trace=${if cfg.trace then "1" else "0"}
    '';

    environment.systemPackages = [
      cfg.package
      (pkgs.callPackage ../pkgs/mortar-gpu-tests { pvr-ddk = cfg.package; })
      cfg.wlegl # mortar-wlegl-demo
      cfg.sdlRun
    ];
  }) ];
}
