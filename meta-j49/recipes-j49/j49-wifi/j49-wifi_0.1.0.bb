SUMMARY = "WiFi setup hotspot for the J49 head unit"
DESCRIPTION = "Opens an open WiFi access point (J49-setup-XXXX) with a DHCP server when no network is \
configured or none could be joined for five minutes, so that the web page of the thermostat \
can be reached to add one; closes it when a network was added. No crate dependencies. \
Source: tools/j49-wifi; see docs/thermostat.md."
HOMEPAGE = "https://github.com/wian007/nest-v2-firmware"
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit cargo systemd

FILESEXTRAPATHS:prepend := "${THISDIR}/../../../tools:${THISDIR}/files:"
SRC_URI = "file://j49-wifi/Cargo.toml \
           file://j49-wifi/Cargo.lock \
           file://j49-wifi/src/main.rs \
           file://j49-wifi.service \
           "

S = "${UNPACKDIR}/j49-wifi"

COMPATIBLE_MACHINE = "nest-j49"

SYSTEMD_SERVICE:${PN} = "j49-wifi.service"
SYSTEMD_AUTO_ENABLE = "enable"

do_install:append() {
	install -d ${D}${systemd_system_unitdir}
	install -m 0644 ${UNPACKDIR}/j49-wifi.service ${D}${systemd_system_unitdir}/
}

RDEPENDS:${PN} += "wpa-supplicant systemd"
