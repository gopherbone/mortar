# PowerVR DDK 1.19@6345021 userspace from TrimUI stock firmware, patched for NixOS.
# Takes the unpatched `gpu` output of mortar's pkgs/trimui-brick-firmware.nix.
{ lib, stdenv, autoPatchelfHook, trimui-brick-firmware }:
stdenv.mkDerivation {
  pname = "pvr-ddk-userspace";
  version = "1.19.6345021";
  src = trimui-brick-firmware.gpu;
  dontUnpack = true;
  nativeBuildInputs = [ autoPatchelfHook ];
  buildInputs = [ stdenv.cc.cc.lib ]; # libstdc++/libgcc_s
  installPhase = ''
    mkdir -p $out/lib
    cp -a $src/lib/. $out/lib/
    chmod -R u+w $out/lib
    # common soname aliases
    ln -sf libGLES_CM.so $out/lib/libGLESv1_CM.so
    ln -sf libGLES_CM.so $out/lib/libGLESv1_CM.so.1
  '';
  # the libs dlopen each other (libIMGegl -> libpvrNULL_WSEGL, libGLESv2, libsrv_um ...)
  # by bare soname; RUNPATH=$out/lib makes that resolve.
  postFixup = ''
    for f in $out/lib/*.so*; do [ -L "$f" ] || patchelf --add-rpath $out/lib "$f"; done
  '';
  meta.license = lib.licenses.unfree;
  meta.platforms = [ "aarch64-linux" ];
}
