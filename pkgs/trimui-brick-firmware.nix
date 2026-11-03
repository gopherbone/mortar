# SPDX-License-Identifier: GPL-2.0-or-later
#
# TrimUI Brick proprietary runtime firmware, extracted from TrimUI's public
# stock firmware release exactly as jkingston/trimui-brick-firmware (1161bc7)
# does it in package/brick-radio and package/brick-gpu:
#
#   https://github.com/trimui/firmware_brick/releases/tag/v1.1.1-20251126
#   TG3040_Brick_firmware_v1.1.1.zip                    (sha256 488db021...)
#     -> trimui_tg3040.awimg (IMAGEWTY v0x300, LZMA zip member)
#        -> rootfs.fex (raw ext4, sha256 023de6fe...)
#           -> individual files, dumped with debugfs (no mount, nothing run)
#
# Outputs:
#   out  firmware tree for hardware.firmware:
#          lib/firmware/xr829/{boot,fw,sdd}_xr829.bin   XR829 Wi-Fi (xradio_wlan)
#          lib/firmware/xr829/fw_xr829_bt.bin           XR829 BT    (hci_xradio)
#          lib/firmware/rgx.{fw,sh}.22.102.54.38        GE8300 FW   (pvrsrvkm)
#   gpu  PowerVR DDK 1.19@6345021 userspace (libEGL, libGLESv2, ...), kept out
#        of `out` so nothing pulls it in by accident; see gpu/share/doc.
#
# Every file is checked against the sha256 from jkingston's files.json; the
# zip itself is the same fixed-output fetchurl as firmware/mainline/scp.nix
# (same name + hash => same store path => one download).
#
# Proprietary, with no licence granting redistribution: never commit the outputs.
# TrimUI publishes these files, and the project has decided they may be hosted in
# the public bare image and cache, which isn't a licence (docs/updates.md, "What
# may be published"). meta.license = unfree; allow it with
#   lib.getName pkg == "trimui-brick-firmware"
{
  lib,
  stdenvNoCC,
  fetchurl,
  python3,
  e2fsprogs,
  writeText,
}:

let
  version = "1.1.1-20251126";

  # target (relative to the output) -> { source path in stock rootfs; sha256 }
  # From package/brick-radio/files.json in jkingston/trimui-brick-firmware.
  radioFiles = {
    "lib/firmware/xr829/boot_xr829.bin" = {
      source = "/lib/firmware/boot_xr829.bin";
      sha256 = "4912ad59f8fcdadba8c10f2ace82f5dce3a4ac37d5e03e5cb814656ce53efff5";
    };
    "lib/firmware/xr829/fw_xr829.bin" = {
      source = "/lib/firmware/fw_xr829.bin";
      sha256 = "f702cd028538830b8c197c90efd76f6de278a04caec8d626d6625d32ab0669e4";
    };
    "lib/firmware/xr829/sdd_xr829.bin" = {
      source = "/lib/firmware/sdd_xr829.bin";
      sha256 = "a504e3517d45b70404495c6a810fd04231a5d70143d9efac9ffd3de4caf14cdd";
    };
    "lib/firmware/xr829/fw_xr829_bt.bin" = {
      source = "/lib/firmware/fw_xr829_bt.bin";
      sha256 = "88d689db801445300a0c3435e07b113824e5f6d6ad9ec75e16e546cff450a7f6";
    };
  };

  # From package/brick-gpu/files.json. BVNC 22.102.54.38 = GE8300; pvrsrvkm
  # requests rgx.fw.22.102.54.38 (then rgx.fw.22.102p.54.38, rgx.fw) and
  # rgx.sh.22.102.54.38 from the firmware root.
  gpuFirmware = {
    "lib/firmware/rgx.fw.22.102.54.38" = {
      source = "/lib/firmware/rgx.fw.22.102.54.38";
      sha256 = "262026355671bc0f2005bd6cf7fa02d4bfa5fe0d3100f185357081cd07fab91c";
    };
    "lib/firmware/rgx.sh.22.102.54.38" = {
      source = "/lib/firmware/rgx.sh.22.102.54.38";
      sha256 = "d1604a4a462ec21ac96cfc7849f98c211db7c95146fdf0cfc26579b519398680";
    };
  };

  gpuUserspace = {
    "lib/libEGL.so.1" = {
      source = "/usr/lib/libEGL.so";
      sha256 = "cd2a2ce9b2593eb956b974ab90558d0c873de7cf1131f58f6cea7b4fda13b3a1";
    };
    "lib/libGLES_CM.so" = {
      source = "/usr/lib/libGLES_CM.so";
      sha256 = "e53d41dfdc8f9cf9527f5d9d940a01b11923ac0a391f6d598a7babd35b6e67c3";
    };
    "lib/libGLESv2.so" = {
      source = "/usr/lib/libGLESv2.so";
      sha256 = "95dd6845c2884c74463a84d11aadcff5f390024807da774dc3de0d1deb35d889";
    };
    "lib/libIMGegl.so" = {
      source = "/usr/lib/libIMGegl.so";
      sha256 = "fa299ae40cb8e9c52ada73330b1648de9a34c2d83fc38f43f5cf936af720d403";
    };
    "lib/libglslcompiler.so" = {
      source = "/usr/lib/libglslcompiler.so";
      sha256 = "d11b684dd70d0839f78dae8afb3cadf57e86c54aea689eff320407990fe77acd";
    };
    "lib/libpvrNULL_WSEGL.so" = {
      source = "/usr/lib/libpvrNULL_WSEGL.so";
      sha256 = "6313de319dac321e7b7e272525e3cbbb0330ac2ebf1d59c467d5aa97b156c833";
    };
    "lib/libsrv_um.so" = {
      source = "/usr/lib/libsrv_um.so";
      sha256 = "6d7991b79dd10fe3fcda561dd886ef66c698fe59ee62d6668191f1f020e7c621";
    };
    "lib/libusc.so" = {
      source = "/usr/lib/libusc.so";
      sha256 = "524bfb2ef820f846872a0d0d907c2aaba48151f5f264c786a5c6f53879fb8d22";
    };
  };

  # "OUTPUT_VAR TARGET SOURCE SHA256" lines for the shell loop below.
  manifest = writeText "trimui-brick-firmware-manifest" (
    lib.concatStrings (
      lib.concatLists (
        lib.mapAttrsToList
          (outVar: files:
            lib.mapAttrsToList (target: r: "${outVar} ${target} ${r.source} ${r.sha256}\n") files)
          {
            out = radioFiles // gpuFirmware;
            gpu = gpuUserspace;
          }
      )
    )
  );

  # Pull rootfs.fex out of the IMAGEWTY image inside the zip. Same parsing and
  # checks as jkingston's support/stock/extract_stock.py (format reference:
  # ChrisHughes/allwinner-image-tool), but streams the 557 MiB member to disk
  # and mmaps it like firmware/mainline/extract-scp.py, instead of reading it
  # into memory. Pure parsing; nothing from the archive is executed.
  extractRootfs = writeText "extract-rootfs.py" ''
    import hashlib, mmap, shutil, struct, sys, zipfile
    from pathlib import Path

    ROOTFS_SHA256 = "023de6fe3d9ef2a268155bcc61c4d2f4e8f17d35da4d17fa815157e69aae12f8"

    archive, out = Path(sys.argv[1]), Path(sys.argv[2])
    tmp = out.with_name(out.name + ".awimg")
    with zipfile.ZipFile(archive) as z, z.open("trimui_tg3040.awimg") as s, open(tmp, "wb") as d:
        shutil.copyfileobj(s, d, 1 << 20)  # zipfile checks the member CRC
    with open(tmp, "rb") as f, mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ) as image:
        if image[:8] != b"IMAGEWTY" or struct.unpack_from("<I", image, 8)[0] != 0x300:
            raise SystemExit("unsupported IMAGEWTY format")
        if struct.unpack_from("<I", image, 24)[0] != len(image):
            raise SystemExit("truncated IMAGEWTY image")
        stride = struct.unpack_from("<I", image, 32)[0]
        count = struct.unpack_from("<I", image, 60)[0]
        if stride < 56 or count > len(image) // stride - 1:
            raise SystemExit("invalid IMAGEWTY record table")
        matches = []
        for index in range(count):
            base = (index + 1) * stride
            size = struct.unpack_from("<I", image, base)[0]
            if size > stride - 56:
                raise SystemExit("invalid IMAGEWTY filename size")
            name = image[base + 36:base + 36 + size].split(b"\0", 1)[0]
            stored, _, original, _, offset = struct.unpack_from("<5I", image, base + 36 + size)
            if offset + stored > len(image) or original > stored:
                raise SystemExit("invalid IMAGEWTY record extent")
            if name == b"rootfs.fex":
                matches.append((offset, stored))
        if len(matches) != 1:
            raise SystemExit("expected exactly one rootfs.fex")
        offset, stored = matches[0]
        h = hashlib.sha256()
        with open(out, "wb") as d:
            for pos in range(offset, offset + stored, 1 << 20):
                chunk = image[pos:min(pos + (1 << 20), offset + stored)]
                h.update(chunk)
                d.write(chunk)
    tmp.unlink()
    if h.hexdigest() != ROOTFS_SHA256:
        raise SystemExit("stock rootfs.fex checksum mismatch")
    print(f"rootfs.fex: {stored} bytes, sha256 {ROOTFS_SHA256}")
  '';
in
stdenvNoCC.mkDerivation {
  pname = "trimui-brick-firmware";
  inherit version;

  outputs = [ "out" "gpu" ];

  # Identical to firmware/mainline/scp.nix's src (fixed-output: same store path).
  src = fetchurl {
    url = "https://github.com/trimui/firmware_brick/releases/download/v1.1.1-20251126/TG3040_Brick_firmware_v1.1.1.zip";
    hash = "sha256-SI2wIXboGtT9z54LtNv0BsiDGZ9bNSFEXgOL+ByJFu8=";
  };

  nativeBuildInputs = [ python3 e2fsprogs ];

  dontUnpack = true;
  dontConfigure = true;
  # No strip/patchelf: keep the blobs byte-identical to the verified hashes.
  dontFixup = true;

  buildPhase = ''
    runHook preBuild
    python3 -I ${extractRootfs} "$src" rootfs.ext4
    runHook postBuild
  '';

  installPhase = ''
    runHook preInstall
    while read -r outVar target source sha; do
      dest="''${!outVar}/$target"
      mkdir -p "$(dirname "$dest")"
      # debugfs reads the image as data (no mount); -R runs one command.
      debugfs -R "dump $source $dest" rootfs.ext4 2>/dev/null
      [ -f "$dest" ] || { echo "missing in stock rootfs: $source" >&2; exit 1; }
      echo "$sha  $dest" | sha256sum -c --quiet - \
        || { echo "checksum mismatch: $target ($source)" >&2; exit 1; }
      chmod 0444 "$dest"
      echo "ok $outVar/$target"
    done < ${manifest}
    rm rootfs.ext4

    # Same soname links jkingston adds (package/brick-gpu/extract.py).
    ln -s libEGL.so.1 $gpu/lib/libEGL.so
    ln -s libGLESv2.so $gpu/lib/libGLESv2.so.2

    install -Dm0644 /dev/stdin $gpu/share/doc/trimui-brick-gpu-userspace/README <<'EOF'
    TrimUI Brick GPU userspace (PowerVR Rogue GE8300, BVNC 22.102.54.38)
    =====================================================================

    Imagination DDK 1.19@6345021 user-mode driver, taken unmodified from
    /usr/lib of TrimUI's stock rootfs (firmware v1.1.1-20251126). It pairs
    with the pvrsrvkm kernel module from jkingston's kernel (img-rogue
    imported from StarFive) and the rgx.fw/rgx.sh.22.102.54.38 firmware in
    this package's main output. Proprietary (docs/updates.md).

      libEGL.so.1 (+ libEGL.so)        EGL front end (IMG, not glvnd)
      libGLESv2.so (+ libGLESv2.so.2)  OpenGL ES 2/3
      libGLES_CM.so                    OpenGL ES 1.x
      libIMGegl.so                     IMG EGL core
      libglslcompiler.so, libusc.so    shader compiler
      libsrv_um.so                     services client (talks to pvrsrvkm)
      libpvrNULL_WSEGL.so              NULL window system: pbuffer only

    Status: packaged, NOT installed system-wide. Limits (brick-port-survey):
      - pbuffer-only rendering (NULL_WSEGL): no KMS scanout, no Wayland/X GL,
        no zero-copy; frames must be read back and presented via DRM/fbdev.
      - Not Mesa; nothing in nixpkgs links against these.
      - The ELF files are unpatched glibc/aarch64 blobs (they expect
        /usr/lib-style lookup of libc/libstdc++/each other). Before use on
        NixOS, wrap them with autoPatchelfHook (glibc + libstdc++) or run
        with LD_LIBRARY_PATH including this lib/ and a libstdc++.
    EOF

    runHook postInstall
  '';

  preferLocalBuild = true;
  allowSubstitutes = false;

  meta = {
    description = "TrimUI Brick XR829 Wi-Fi/Bluetooth and GE8300 GPU firmware (+ GPU userspace in the gpu output), from stock firmware";
    homepage = "https://github.com/trimui/firmware_brick";
    license = lib.licenses.unfree;
    sourceProvenance = [ lib.sourceTypes.binaryFirmware lib.sourceTypes.binaryNativeCode ];
    hydraPlatforms = [ ];
    platforms = lib.platforms.all;
  };
}
