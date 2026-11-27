SUMMARY = "MQTT bridge for the J49 thermostat with Home Assistant discovery"
DESCRIPTION = "Publishes the thermostat state (files of bpd and /var/lib/j49) to an MQTT \
broker and applies commands from it; announces itself to Home Assistant. No crate \
dependencies. Source lives in tools/j49-mqtt in this repository; see docs/thermostat.md."
HOMEPAGE = "https://github.com/wian007/nest-yocto"
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit cargo systemd

FILESEXTRAPATHS:prepend := "${THISDIR}/../../../tools:${THISDIR}/files:"
SRC_URI = "file://j49-mqtt/Cargo.toml \
           file://j49-mqtt/Cargo.lock \
           file://j49-mqtt/src/main.rs \
           file://lang/en.txt \
           file://lang/nl.txt \
           file://j49-mqtt.service \
           "

S = "${UNPACKDIR}/j49-mqtt"

COMPATIBLE_MACHINE = "nest-j49"

SYSTEMD_SERVICE:${PN} = "j49-mqtt.service"
SYSTEMD_AUTO_ENABLE = "enable"

do_install:append() {
	install -d ${D}${systemd_system_unitdir}
	install -m 0644 ${UNPACKDIR}/j49-mqtt.service ${D}${systemd_system_unitdir}/
}
