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
  };

  config = lib.mkIf cfg.enable {
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
    ];
  };
}
