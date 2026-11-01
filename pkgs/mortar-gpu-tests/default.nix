# GPU test programs linked against the patched stock DDK userspace, and
# drm-grab (panel screenshot as PNG; no GPU).
{ stdenv, libdrm, libglvnd, pkg-config, zlib, pvr-ddk }:
stdenv.mkDerivation {
  pname = "mortar-gpu-tests";
  version = "0";
  src = ./.;
  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ libdrm zlib ];
  # Khronos headers only (from libglvnd's dev output); link against the IMG libs.
  buildPhase = ''
    I="-I${libglvnd.dev}/include $(pkg-config --cflags libdrm)"
    L="-L${pvr-ddk}/lib -Wl,-rpath,${pvr-ddk}/lib -lEGL -lGLESv2"
    $CC -O2 -Wall -o egl-pbuffer egl-pbuffer.c $I $L
    $CC -O2 -Wall -o egl-window egl-window.c $I $L -lm
    $CC -O2 -Wall -o kms-gles kms-gles.c $I $L $(pkg-config --libs libdrm)
    $CC -O2 -Wall -o devmem devmem.c
    $CC -O2 -Wall -o drm-grab drm-grab.c $(pkg-config --cflags --libs libdrm zlib)
  '';
  installPhase = "mkdir -p $out/bin; cp egl-pbuffer egl-window kms-gles drm-grab devmem $out/bin/";
}
