SUMMARY = "Web page for the J49 thermostat, login confirmed on the device"
DESCRIPTION = "Serves a page on port 80 to set temperature, away, schedule and the MQTT \
broker. Login without a password: j49-screen shows a code and somebody at the device allows \
it with the ring. No crate dependencies. Source lives in tools/j49-web; see docs/thermostat.md."
HOMEPAGE = "https://github.com/wian007/nest-v2-firmware"
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit cargo systemd

FILESEXTRAPATHS:prepend := "${THISDIR}/../../../tools:${THISDIR}/files:"
SRC_URI = "file://j49-web/Cargo.toml \
           file://j49-web/Cargo.lock \
           file://j49-web/src/main.rs \
           file://j49-web/src/page.html \
           file://lang/en.txt \
           file://lang/nl.txt \
           file://themes/default.theme \
           file://themes/home-assistant.theme \
           file://themes/home-assistant-light.theme \
           file://themes/light.theme \
           file://j49-web.service \
           "

S = "${UNPACKDIR}/j49-web"

COMPATIBLE_MACHINE = "nest-j49"

SYSTEMD_SERVICE:${PN} = "j49-web.service"
SYSTEMD_AUTO_ENABLE = "enable"

do_install:append() {
	install -d ${D}${systemd_system_unitdir}
	install -m 0644 ${UNPACKDIR}/j49-web.service ${D}${systemd_system_unitdir}/
}
