SUMMARY = "Sleep manager for the J49 head unit"
DESCRIPTION = "Suspends the head unit when the panel is off, the heating is idle and nobody \
uses SSH or the web page: backplate quiet mode through bpd, wake-on-WLAN patterns, suspend. \
Off by default. No crate dependencies. Source: tools/j49-sleep; see docs/sleep-j49.md."
HOMEPAGE = "https://github.com/wian007/nest-yocto"
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit cargo systemd

FILESEXTRAPATHS:prepend := "${THISDIR}/../../../tools:${THISDIR}/files:"
SRC_URI = "file://j49-sleep/Cargo.toml \
           file://j49-sleep/Cargo.lock \
           file://j49-sleep/src/main.rs \
           file://j49-sleep.service \
           "

S = "${UNPACKDIR}/j49-sleep"

COMPATIBLE_MACHINE = "nest-j49"

SYSTEMD_SERVICE:${PN} = "j49-sleep.service"
SYSTEMD_AUTO_ENABLE = "enable"

do_install:append() {
	install -d ${D}${systemd_system_unitdir}
	install -m 0644 ${UNPACKDIR}/j49-sleep.service ${D}${systemd_system_unitdir}/
}

# the wake patterns are set with iw
RDEPENDS:${PN} += "iw"
