#!/usr/bin/env bash
# build-dtb-variant.sh NAME 'dts-fragment'  -> zImage-dtb-kx-NAME (same zImage, DTS + fragment)
set -euo pipefail
SP="${SP:-/tmp/j49-kbuild}"
SRC="$SP/linux-6.6.64"; OUT="$SP/out"; CROSS=armv7l-unknown-linux-gnueabihf-
REPO="$(cd "$(dirname "$0")/../.." && pwd)"; NAME=$1; FRAG=$2
D="$SRC/arch/arm/boot/dts/ti/omap/omap3-nest-j49-$NAME.dts"
{ cat "$REPO/meta-j49/recipes-kernel/linux/files/omap3-nest-j49.dts"; echo; echo "/* variant $NAME */"; echo "$FRAG"; } > "$D"
nix shell nixpkgs#pkgsCross.armv7l-hf-multiplatform.buildPackages.gcc nixpkgs#gnumake nixpkgs#bison nixpkgs#flex nixpkgs#bc \
  nixpkgs#which nixpkgs#coreutils nixpkgs#gawk nixpkgs#findutils nixpkgs#perl --command bash -eu -c "
    make -C '$SRC' O='$OUT' ARCH=arm CROSS_COMPILE='$CROSS' ti/omap/omap3-nest-j49-$NAME.dtb 2>&1 | tail -2"
cat "$OUT/arch/arm/boot/zImage" "$OUT/arch/arm/boot/dts/ti/omap/omap3-nest-j49-$NAME.dtb" > "$SP/zImage-dtb-$NAME"
python3 - "$SP/zImage-dtb-$NAME" "$SP/zImage-dtb-kx-$NAME" <<'PY'
import struct,sys
d=bytearray(open(sys.argv[1],'rb').read())
while len(d)%4: d.append(0)
d[0x2c:0x30]=struct.pack('<I',len(d)); open(sys.argv[2],'wb').write(d)
PY
ls -la "$SP/zImage-dtb-kx-$NAME"
