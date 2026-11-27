SUMMARY = "Update of the J49 root file system over the network, with the old system as the way back"
DESCRIPTION = "Streams a new root file system into the slot that is not running, checks it, asks \
nandio for a trial start of it and confirms it when it is up; else the old system starts again. \
Nothing by itself: checking for a version is off unless switched on, installing is always an \
order. No crate dependencies. Source: tools/j49-update; see docs/update.md."
HOMEPAGE = "https://github.com/wian007/nest-v2-firmware"
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit cargo systemd

FILESEXTRAPATHS:prepend := "${THISDIR}/../../../tools:${THISDIR}/files:"
SRC_URI = "file://j49-update/Cargo.toml \
           file://j49-update/Cargo.lock \
           file://j49-update/src/main.rs \
           file://j49-update-confirm.service \
           file://j49-update-confirm.timer \
           file://j49-update-check.service \
           file://j49-update-check.timer \
           "

S = "${UNPACKDIR}/j49-update"

COMPATIBLE_MACHINE = "nest-j49"

SYSTEMD_SERVICE:${PN} = "j49-update-confirm.timer j49-update-check.timer"
SYSTEMD_AUTO_ENABLE = "enable"

do_install:append() {
	install -d ${D}${systemd_system_unitdir}
	for u in j49-update-confirm.service j49-update-confirm.timer j49-update-check.service j49-update-check.timer; do
		install -m 0644 ${UNPACKDIR}/$u ${D}${systemd_system_unitdir}/
	done
}

FILES:${PN} += "${systemd_system_unitdir}"
# https with curl; the flash memory with mtd-utils
RDEPENDS:${PN} += "curl ca-certificates mtd-utils"
