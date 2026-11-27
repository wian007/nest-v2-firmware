# Minimal wpa_supplicant for the Nest: nl80211 + WPA2-PSK, internal crypto.
#
# The default build (full defconfig: EAP, P2P, mesh, D-Bus, OpenSSL) segfaults at start-up on
# this board, right after the TDLS capability lines, with any config file. The build that works
# is the one in tools/kernel-firstlight/build-wpa-min.sh; this is the same configuration for
# Yocto. No OpenSSL also drops libcrypto/libssl from the image (about 4.7 MB of the 46 MiB root
# slot) unless something else pulls them in.
PACKAGECONFIG = ""

wpa_supplicant_write_config () {
	cat > wpa_supplicant/.config <<EOF
CONFIG_DRIVER_NL80211=y
CONFIG_LIBNL32=y
CONFIG_CTRL_IFACE=y
CONFIG_BACKEND=file
CONFIG_TLS=internal
CONFIG_INTERNAL_LIBTOMMATH=y
CONFIG_NO_RANDOM_POOL=y
CONFIG_IEEE80211W=y
CONFIG_NO_CONFIG_WRITE=y
CONFIG_NO_CONFIG_BLOBS=y
CONFIG_NO_ROAMING=y
CONFIG_NO_WPA_PASSPHRASE=y
EOF
}
