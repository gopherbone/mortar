# GPU test programs linked against the patched stock DDK userspace.
{ stdenv, libdrm, libglvnd, pkg-config, pvr-ddk }:
stdenv.mkDerivation {
  pname = "mortar-gpu-tests";
  version = "0";
  src = ./.;
  nativeBuildInputs = [ pkg-config ];
  buildInputs = [ libdrm ];
  # Khronos headers only (from libglvnd's dev output); link against the IMG libs.
  buildPhase = ''
    I="-I${libglvnd.dev}/include $(pkg-config --cflags libdrm)"
    L="-L${pvr-ddk}/lib -Wl,-rpath,${pvr-ddk}/lib -lEGL -lGLESv2"
    $CC -O2 -Wall -o egl-pbuffer egl-pbuffer.c $I $L
    $CC -O2 -Wall -o egl-window egl-window.c $I $L -lm
    $CC -O2 -Wall -o kms-gles kms-gles.c $I $L $(pkg-config --libs libdrm)
    $CC -O2 -Wall -o devmem devmem.c
  '';
  installPhase = "mkdir -p $out/bin; cp egl-pbuffer egl-window kms-gles devmem $out/bin/";
}
