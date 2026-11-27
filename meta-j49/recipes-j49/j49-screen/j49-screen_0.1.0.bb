SUMMARY = "Thermostat screen for the Nest J49 round panel"
DESCRIPTION = "Draws the thermostat on the 320x320 panel (/dev/fb0): a dial with the target \
temperature, the room temperature and what the heating does; a menu for mode, schedule, away, \
status and settings; the login question of the web page; the installer's progress. \
Source lives in tools/j49-screen in this repository."
HOMEPAGE = "https://github.com/wian007/nest-v2-firmware"
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit cargo systemd

# Only the crate sources cargo needs from tools/ (not the local target/ dir). Cargo.toml/Cargo.lock are
# the device variants in files/ (no minifb); crate dependencies are vendored by bitbake.
FILESEXTRAPATHS:prepend := "${THISDIR}/../../../tools:${THISDIR}/files:"
SRC_URI = "file://j49-screen/Cargo.toml \
           file://j49-screen/Cargo.lock \
           file://j49-screen/src/main.rs \
           file://lang/en.txt \
           file://lang/nl.txt \
           file://themes/default.theme \
           file://themes/home-assistant.theme \
           file://themes/light.theme \
           file://j49-screen.service \
           crate://crates.io/libc/0.2.191 \
           "
SRC_URI[libc-0.2.191.sha256sum] = "65cd87c9a6d60536580b316ad985726104730a5151c99845f2490428ef91240a"

S = "${UNPACKDIR}/j49-screen"

COMPATIBLE_MACHINE = "nest-j49"

SYSTEMD_SERVICE:${PN} = "j49-screen.service"
SYSTEMD_AUTO_ENABLE = "enable"

do_install:append() {
	install -d ${D}${systemd_system_unitdir}
	install -m 0644 ${UNPACKDIR}/j49-screen.service ${D}${systemd_system_unitdir}/
	# texts per language and themes, also read by j49-web and j49-mqtt
	install -d ${D}${datadir}/j49/lang ${D}${datadir}/j49/themes
	install -m 0644 ${UNPACKDIR}/lang/*.txt ${D}${datadir}/j49/lang/
	install -m 0644 ${UNPACKDIR}/themes/*.theme ${D}${datadir}/j49/themes/
}

FILES:${PN} += "${datadir}/j49"
