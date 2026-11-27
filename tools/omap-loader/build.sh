#!/usr/bin/env bash
# Build the host-side OMAP ROM USB peripheral-boot pusher (vendored omap_loader.c)
# with a proper Nix toolchain + libusb — no patchelf hacks, no stray glibc paths.
#
# Usage: build.sh [out-dir]   (default: ./out)
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:-$HERE/out}"
mkdir -p "$OUT"
LIBUSB_DEV=$(nix build --no-link --print-out-paths nixpkgs#libusb1.dev)
nix shell nixpkgs#gcc nixpkgs#libusb1 nixpkgs#pkg-config nixpkgs#gnumake --command bash -c "
  export PKG_CONFIG_PATH='$LIBUSB_DEV/lib/pkgconfig'
  cd '$HERE'
  make clean || true
  make CC=gcc
  cp omap_loader '$OUT/omap_loader'
"
echo "=== built ==="; ls -la "$OUT/omap_loader"
"$OUT/omap_loader" --help || true
