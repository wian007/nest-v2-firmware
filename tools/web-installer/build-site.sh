#!/usr/bin/env bash
# Put the installer page together as a static site: the page, its texts, the themes of the
# thermostat (tools/themes) and, with --images, the image files with a manifest of checksums.
#
# Usage: build-site.sh <output directory> [--images]
#
# Serve the directory over https, or locally (WebUSB works on http://localhost):
#   python3 -m http.server 8049 -d <output directory>     then open http://localhost:8049/
#
# Without --images the page asks the user to choose each file. With it the page loads them by
# itself; the files are taken from where the build scripts put them:
#   x-load.bin, u-boot.bin      tools/dfu-install/nest-dfu/        (see its README)
#   nest-service-uImage         tools/dfu-install/payload/         (installer/build-service-kernel.sh)
#   u-boot-nest-j49.uimg        tools/dfu-install/uboot/out/       (uboot/build.sh)
#   zImage-installer, uImage-j49-a   tools/dfu-install/installer/out/   (installer/build.sh)
#   the root file system        J49_ROOTFS=<file .jffs2>, or the newest in build/tmp/deploy
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="${1:?usage: build-site.sh <output directory> [--images]}"
mkdir -p "$OUT/js" "$OUT/lang" "$OUT/themes"
cp "$HERE/index.html" "$OUT/"
cp "$HERE"/js/*.js "$OUT/js/"
cp "$HERE"/lang/*.txt "$OUT/lang/"
cp "$HERE"/../themes/*.theme "$OUT/themes/"
if [ "${2:-}" = "--images" ]; then
    D="$HERE/../dfu-install"
    ROOTFS="${J49_ROOTFS:-$(ls -1t "$HERE"/../../build/tmp/deploy/images/nest-j49/*.rootfs.jffs2 2>/dev/null | head -n 1)}"
    mkdir -p "$OUT/images"
    {
        echo "{"
        sep=""
        for pair in "xload:$D/nest-dfu/x-load.bin" "ubootram:$D/nest-dfu/u-boot.bin" "service:$D/payload/nest-service-uImage" \
                    "uboot:$D/uboot/out/u-boot-nest-j49.uimg" "installer:$D/installer/out/zImage-installer" \
                    "kernel:$D/installer/out/uImage-j49-a" "rootfs:$ROOTFS"; do
            role="${pair%%:*}"; src="${pair#*:}"
            [ -f "$src" ] || { echo "missing for $role: $src" >&2; exit 1; }
            name="$(basename "$src")"
            cp -L "$src" "$OUT/images/$name"
            printf '%s  "%s": {"file": "%s", "size": %s, "sha256": "%s"}' "$sep" "$role" "$name" "$(stat -L -c %s "$src")" "$(sha256sum "$src" | cut -d' ' -f1)"
            sep=$',\n'
        done
        printf '\n}\n'
    } > "$OUT/images/manifest.json"
    echo "images: $(du -sh "$OUT/images" | cut -f1)"
fi
echo "site in $OUT"
