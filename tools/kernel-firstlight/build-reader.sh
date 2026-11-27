#!/usr/bin/env bash
# Cross-build the ramoops /dev/mem reader (static-musl armv7, via nixpkgs cross
# gcc — not zig). Output: <workdir>/ramoops-read.
# NOTE: ramoops-read.rs holds the same logic in Rust (preferred per project
# convention) for when a light Rust cross toolchain is available; nixpkgs
# rustc-cross currently needs a full bootstrap, so C is used here to match the
# proven kexec-tools/busybox build path.
set -euo pipefail
WORK="${1:?usage: build-reader.sh <workdir>}"
HERE="$(cd "$(dirname "$0")" && pwd)"
nix shell 'nixpkgs#pkgsCross.armv7l-hf-multiplatform.pkgsStatic.buildPackages.gcc' --command \
  armv7l-unknown-linux-musleabihf-gcc -O2 -static \
  -o "$WORK/ramoops-read" "$HERE/ramoops-read.c"
echo "built $WORK/ramoops-read"
