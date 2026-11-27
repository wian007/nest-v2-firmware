#!/usr/bin/env bash
# Minimal static wpa_supplicant for the initramfs: nl80211 + WPA2-PSK only, internal crypto (no OpenSSL),
# no EAP/TLS/P2P/AP/dbus. Built from the nixpkgs hostap source with the armv7 musl cross toolchain.
set -euo pipefail
cd "${WPA_BUILD_DIR:-$(mktemp -d)}"
SRC=$(nix build --no-link --print-out-paths 'nixpkgs#wpa_supplicant.src')
ALL=$(nix build --no-link --print-out-paths 'nixpkgs#pkgsCross.armv7l-hf-multiplatform.pkgsStatic.libnl^*')
LIBNL=$(echo "$ALL" | grep -vE -- '-(bin|man|dev)$' | head -1)
LIBNLDEV=$(echo "$ALL" | grep -E -- '-dev$' | head -1); [ -n "$LIBNLDEV" ] || LIBNLDEV=$LIBNL
ls "$LIBNL/lib" | head -5; ls "$LIBNLDEV/include" 2>/dev/null | head -3
echo "src=$SRC libnl=$LIBNL dev=$LIBNLDEV"
rm -rf src && mkdir src && tar -xf "$SRC" -C src --strip-components=1
cd src/wpa_supplicant
cat > .config <<'CFG'
CONFIG_DRIVER_NL80211=y
CONFIG_LIBNL32=y
CONFIG_CTRL_IFACE=y
CONFIG_DEBUG_FILE=y
CONFIG_BACKEND=file
CONFIG_TLS=internal
CONFIG_INTERNAL_LIBTOMMATH=y
CONFIG_NO_RANDOM_POOL=y
CONFIG_IEEE80211W=y
CONFIG_NO_CONFIG_WRITE=y
CONFIG_NO_CONFIG_BLOBS=y
CONFIG_NO_ROAMING=y
CONFIG_NO_WPA_PASSPHRASE=y
CFG
nix shell nixpkgs#pkgsCross.armv7l-hf-multiplatform.pkgsStatic.stdenv.cc nixpkgs#gnumake nixpkgs#pkg-config --command bash -c '
  set -e
  CC=$(ls $(dirname $(command -v armv7l-unknown-linux-musleabihf-gcc))/armv7l-unknown-linux-musleabihf-gcc)
  export CC
  export PKG_CONFIG_PATH='"$LIBNLDEV"'/lib/pkgconfig:'"$LIBNL"'/lib/pkgconfig
  export CFLAGS="-Os -I'"$LIBNLDEV"'/include/libnl3 -I'"$LIBNL"'/include/libnl3" LDFLAGS="-static -L'"$LIBNL"'/lib"
  export LIBS="-lnl-genl-3 -lnl-3 -lpthread -lm" LIBS_c="-lnl-genl-3 -lnl-3"
  make -j$(nproc) V=0 wpa_supplicant wpa_cli 2>&1 | tail -5
  $(dirname $CC)/armv7l-unknown-linux-musleabihf-strip wpa_supplicant wpa_cli
  ls -la wpa_supplicant wpa_cli; file wpa_supplicant | cut -c1-120
'
