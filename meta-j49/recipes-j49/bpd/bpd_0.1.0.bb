SUMMARY = "Nest J49 backplate link daemon: head-unit watchdog keepalive + frame tools"
DESCRIPTION = "Feeds the backplate STM32's head-unit watchdog with the type-0xa1 \
'quiet message' frame on UART3 (/dev/ttyS2); without it the backplate cold-resets \
the head unit ~29 min after boot. Also a sniffer/sender for the backplate protocol. \
Source lives in tools/bpd in this repository."
HOMEPAGE = "https://github.com/wian007/nest-yocto"
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit cargo systemd

# The crate is part of this repo (tools/bpd); only the files cargo needs, not
# the local target/ directory. Crate dependencies are vendored by bitbake from
# Cargo.lock (update the crate:// line + checksum when the lock changes).
FILESEXTRAPATHS:prepend := "${THISDIR}/../../../tools:${THISDIR}/files:"
SRC_URI = "file://bpd/Cargo.toml \
           file://bpd/Cargo.lock \
           file://bpd/src/main.rs \
           file://bpd.service \
           crate://crates.io/libc/0.2.191 \
           "
SRC_URI[libc-0.2.191.sha256sum] = "65cd87c9a6d60536580b316ad985726104730a5151c99845f2490428ef91240a"

S = "${UNPACKDIR}/bpd"

COMPATIBLE_MACHINE = "nest-j49"

SYSTEMD_SERVICE:${PN} = "bpd.service"
SYSTEMD_AUTO_ENABLE = "enable"

do_install:append() {
	install -d ${D}${systemd_system_unitdir}
	install -m 0644 ${UNPACKDIR}/bpd.service ${D}${systemd_system_unitdir}/
}
