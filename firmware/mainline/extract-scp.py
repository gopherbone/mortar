#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
#
# Extract and verify the TrimUI Brick SCP (ARISC) firmware and its service
# device tree from the stock firmware image (trimui_tg3040.awimg, inside
# TrimUI's TG3040_Brick_firmware_v1.1.1.zip).
#
# Adapted from package/brick-scp/extract.py in
# https://github.com/jkingston/trimui-brick-firmware (1161bc7, GPL-2.0).
# Same parsing, sizes and hashes; changed to stream the 557 MiB .awimg out of
# the zip (LZMA, method 14, which Info-ZIP unzip cannot read) to a scratch
# file and mmap it, instead of holding it in memory.
#
# Usage: extract-scp.py FIRMWARE.zip OUTDIR  ->  OUTDIR/scp.bin, OUTDIR/service.dtb
import hashlib
import mmap
import shutil
import struct
import sys
import zipfile
from pathlib import Path

ARCHIVE_IMAGE = "trimui_tg3040.awimg"

BOOT_PACKAGE = "boot_package.fex"
BOOT_PACKAGE_SHA256 = "9e3ae29b54a8c87f73b8168764cc24a3f77a9aa7a8873721c3122937aaad5a31"
ARTIFACTS = {
    # name: (size, sha256, output file)
    "scp": (0x14008, "b25eacb050bc5c19245bb3b032f8851f83bcd4e634bb7111aee811bd5ba8bbe8", "scp.bin"),
    "dtb": (0x25600, "06c820c4fe4df1efb454fb5794e7e8a185956cb660ad33711c365af66a5cf7d8", "service.dtb"),
}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def imagewty_record(image, wanted):
    if image[:8] != b"IMAGEWTY" or struct.unpack_from("<I", image, 8)[0] != 0x300:
        raise ValueError("unsupported IMAGEWTY format")
    if struct.unpack_from("<I", image, 24)[0] != len(image):
        raise ValueError("truncated IMAGEWTY image")
    stride = struct.unpack_from("<I", image, 32)[0]
    count = struct.unpack_from("<I", image, 60)[0]
    if stride < 56 or count > len(image) // stride - 1:
        raise ValueError("invalid IMAGEWTY table")
    matches = []
    for index in range(count):
        base = (index + 1) * stride
        name_size = struct.unpack_from("<I", image, base)[0]
        if name_size > stride - 56:
            raise ValueError("invalid IMAGEWTY filename")
        name = image[base + 36:base + 36 + name_size].split(b"\0", 1)[0]
        stored, _, original, _, offset = struct.unpack_from("<5I", image, base + 36 + name_size)
        if offset + stored > len(image) or original > stored:
            raise ValueError("invalid IMAGEWTY record")
        if name == wanted.encode():
            matches.append(image[offset:offset + stored])
    if len(matches) != 1:
        raise ValueError(f"expected one {wanted}")
    return matches[0]


def toc_records(package):
    if package[:13] != b"sunxi-package" or struct.unpack_from("<I", package, 16)[0] != 0x89119800:
        raise ValueError("unsupported boot package")
    count, valid = struct.unpack_from("<II", package, 32)
    if count != 4 or valid > len(package) or 64 + count * 368 > 2048:
        raise ValueError("invalid boot package table")
    records = {}
    for index in range(count):
        base = 64 + index * 368
        if package[base + 364:base + 368] != b"IIE;":
            raise ValueError("invalid boot package record")
        name = package[base:base + 64].split(b"\0", 1)[0].decode("ascii")
        offset, size = struct.unpack_from("<II", package, base + 64)
        if offset < 2048 or offset + size > valid or name in records:
            raise ValueError("invalid boot package extent")
        records[name] = package[offset:offset + size]
    if list(records) != ["u-boot", "monitor", "scp", "dtb"]:
        raise ValueError("unexpected boot package contents")
    return records


def main(archive, destination):
    image_path = destination / (ARCHIVE_IMAGE + ".tmp")
    destination.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(archive) as source, source.open(ARCHIVE_IMAGE) as src, \
            open(image_path, "wb") as dst:
        shutil.copyfileobj(src, dst, 1 << 20)
    with open(image_path, "rb") as f, mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ) as image:
        package = bytes(imagewty_record(image, BOOT_PACKAGE))
    image_path.unlink()
    if digest(package) != BOOT_PACKAGE_SHA256:
        raise ValueError("boot package checksum mismatch")
    records = toc_records(package)
    for name, (size, expected, filename) in ARTIFACTS.items():
        data = records[name]
        if len(data) != size or digest(data) != expected:
            raise ValueError(f"{name} checksum mismatch")
        (destination / filename).write_bytes(data)
        print(f"{filename}: {size:#x} bytes, sha256 {expected}")


if __name__ == "__main__":
    if len(sys.argv) != 3:
        raise SystemExit("usage: extract-scp.py FIRMWARE.zip OUTDIR")
    main(Path(sys.argv[1]), Path(sys.argv[2]))
