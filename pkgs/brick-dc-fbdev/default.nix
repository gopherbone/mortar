# Out-of-tree build of brick_dc_fbdev.ko against Mortar's kernel.
# Wired in by modules/brick-gpu.nix.
{ lib, stdenv, kernel }:
stdenv.mkDerivation {
  pname = "brick-dc-fbdev";
  version = "0.1";
  src = ./.;
  nativeBuildInputs = kernel.moduleBuildDependencies;
  # DDK headers live in the kernel *source* tree (drivers/gpu/drm/img/img-rogue).
  postUnpack = ''
    mkdir -p $sourceRoot/mod
    cp $sourceRoot/brick_dc_fbdev.c $sourceRoot/mod/
    cat > $sourceRoot/mod/Kbuild <<KB
    IMG := ${kernel.src}/drivers/gpu/drm/img
    obj-m += brick_dc_fbdev.o
    ccflags-y += -DPVR_PLATFORM_BRICK -include \$(IMG)/img-rogue/config_kernel.h -I\$(IMG) -I\$(IMG)/img-rogue/include -I\$(IMG)/img-rogue/services/include -I\$(IMG)/img-rogue/services/server/include -I\$(IMG)/img-rogue/include/public -I\$(IMG)/img-rogue/hwdefs/rogue/km
    KB
  '';
  buildPhase = ''
    make -C ${kernel.dev}/lib/modules/${kernel.modDirVersion}/build M=$PWD/mod modules
  '';
  # Install through kbuild's modules_install: it signs the module and writes
  # the brick_dc_fbdev.ko.cer certificate nixpkgs' aggregateModules expects
  # (same approach as nixpkgs' broadcom-sta).
  installPhase = ''
    make -C ${kernel.dev}/lib/modules/${kernel.modDirVersion}/build M=$PWD/mod \
      INSTALL_MOD_PATH=$out INSTALL_MOD_DIR=extra DEPMOD=true modules_install
    # The Brick kernel has CONFIG_MODULE_SIG off, so kbuild writes no
    # certificate, but aggregateModules cats one per external module.
    cer=$out/lib/modules/${kernel.modDirVersion}/extra/brick_dc_fbdev.ko.cer
    [ -e "$cer" ] || touch "$cer"
  '';
  meta.license = lib.licenses.gpl2Only;
  meta.platforms = [ "aarch64-linux" ];
}
