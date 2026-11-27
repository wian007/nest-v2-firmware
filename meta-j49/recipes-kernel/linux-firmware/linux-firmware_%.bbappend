# linux-firmware-wl12xx ships every WL127x/WL128x firmware generation (7 MB). The J49 has a
# WL1271 driven by wlcore, which loads wl127x-fw-5-sr.bin (single role; -mr is multi-role,
# -plt production-line test). Keep only the current-generation files, plus the generic NVS
# pair that ${PN}-wlcommon (a hard RDEPENDS of -wl12xx) is made of. The TIInit_*.bts files
# are Bluetooth init scripts; the J49 has no BT radio.
do_install:append() {
    for f in ${D}${nonarch_base_libdir}/firmware/ti-connectivity/*; do
        case "$(basename "$f")" in
            wl127x-fw-5-sr.bin|wl127x-fw-5-mr.bin|wl127x-nvs.bin|wl1271-nvs.bin) ;;
            *) rm -f "$f" ;;
        esac
    done
}
