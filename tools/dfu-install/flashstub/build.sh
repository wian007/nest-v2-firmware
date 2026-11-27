#!/usr/bin/env bash
# build.sh -- cross-build the DFU flashstub (bare-metal, linked 0x80100000)
# with the same nixpkgs static-musl cross gcc the other device helpers use
# (the pattern from tools/kernel-firstlight/build-reader.sh; Rust is the
# project convention but the stub runs before anything can host it).
# Output: out/flashstub.bin (raw ARM code at offset 0, like u-boot.bin --
# that is what the vendored x-load receives and jumps to).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:-$HERE/out}"; mkdir -p "$OUT"
# NEST_NO_PANEL=1 -> blind stub (no display bring-up), for bisecting a hang
EXTRA_CFLAGS="${NEST_NO_PANEL:+-DNEST_NO_PANEL}"

nix shell nixpkgs#pkgsCross.armv7l-hf-multiplatform.buildPackages.gcc \
  nixpkgs#pkgsCross.armv7l-hf-multiplatform.buildPackages.binutils \
  --command bash -eu -c "
    cd '$HERE'
    CC=armv7l-unknown-linux-gnueabihf-gcc
    \$CC -c start.S -o start.o                # the ld script pins start.o first
    \$CC -O2 -marm -march=armv7-a -mfloat-abi=soft \
        -ffreestanding -fno-builtin -fno-stack-protector -fno-common \
        -fno-unwind-tables -fno-asynchronous-unwind-tables \
        -Wall -Wextra -Wno-unused-parameter \
        $EXTRA_CFLAGS \
        -nostdlib -nostartfiles -T flashstub.ld \
        start.o stub.c nand.c panel.c -o '$OUT/flashstub.elf' -lgcc
    armv7l-unknown-linux-gnueabihf-objcopy -O binary \
        '$OUT/flashstub.elf' '$OUT/flashstub.bin'
  "

SZ=$(stat -c %s "$OUT/flashstub.bin")
echo "=== $OUT/flashstub.bin ($SZ bytes at 0x80100000) ==="
if [ "$SZ" -gt $((512 * 1024)) ]; then
    echo "WARNING: stub larger than half a MB -- check what you linked in" >&2
    exit 1
fi
