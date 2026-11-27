SUMMARY = "Nest J49 system configuration: WiFi station via systemd-networkd + wpa_supplicant, watchdog"
DESCRIPTION = "Config-only package. The network secrets (wpa_supplicant-wlan0.conf, SSH keys, the \
device NVS) are NOT part of the image; they are provisioned onto the device separately."
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

SRC_URI = "file://80-wlan0.network \
           file://10-j49-watchdog.conf \
           file://10-j49-mem.conf \
           file://20-j49-timeouts.conf \
           file://10-j49-journald.conf \
           file://j49-wlan-reprobe.service \
           file://j49-wlan-reprobe \
           file://j49-release \
           file://10-j49-local-ntp.conf \
           file://j49-ntp-gateway \
           file://j49-ntp-gateway.service \
           "
S = "${UNPACKDIR}"

inherit allarch

RDEPENDS:${PN} = "systemd wpa-supplicant"

do_install() {
	install -d ${D}${systemd_unitdir}/network
	install -m 0644 ${UNPACKDIR}/80-wlan0.network ${D}${systemd_unitdir}/network/

	install -d ${D}${sysconfdir}/sysctl.d
	install -m 0644 ${UNPACKDIR}/10-j49-mem.conf ${D}${sysconfdir}/sysctl.d/

	install -d ${D}${sysconfdir}/systemd/system.conf.d
	install -m 0644 ${UNPACKDIR}/10-j49-watchdog.conf ${UNPACKDIR}/20-j49-timeouts.conf ${D}${sysconfdir}/systemd/system.conf.d/

	# wpa_supplicant@wlan0 reads /etc/wpa_supplicant/wpa_supplicant-wlan0.conf (provisioned,
	# not shipped). Enable the instance here; systemd.bbclass cannot enable another
	# package's template instance.
	install -d ${D}${sysconfdir}/systemd/system/multi-user.target.wants
	ln -sf ${systemd_system_unitdir}/wpa_supplicant@.service \
		${D}${sysconfdir}/systemd/system/multi-user.target.wants/wpa_supplicant@wlan0.service
	install -d ${D}${sysconfdir}/wpa_supplicant

	install -d ${D}${sysconfdir}/systemd/journald.conf.d
	install -m 0644 ${UNPACKDIR}/10-j49-journald.conf ${D}${sysconfdir}/systemd/journald.conf.d/

	# wl12xx is built in and probes before / is mounted, so its NVS request fails and the
	# WiFi firmware never boots; re-probe it once local-fs is up (see the unit for details).
	# `j49-release`: commit of the root filesystem + the live slot and kernel version
	install -d ${D}${bindir}
	install -m 0755 ${UNPACKDIR}/j49-release ${D}${bindir}/

	install -d ${D}${libexecdir}
	install -m 0755 ${UNPACKDIR}/j49-wlan-reprobe ${D}${libexecdir}/
	# time from the local network only: no public NTP servers, the router as the server
	install -m 0755 ${UNPACKDIR}/j49-ntp-gateway ${D}${libexecdir}/
	install -d ${D}${sysconfdir}/systemd/timesyncd.conf.d
	install -m 0644 ${UNPACKDIR}/10-j49-local-ntp.conf ${D}${sysconfdir}/systemd/timesyncd.conf.d/
	install -d ${D}${systemd_system_unitdir}
	install -m 0644 ${UNPACKDIR}/j49-wlan-reprobe.service ${UNPACKDIR}/j49-ntp-gateway.service ${D}${systemd_system_unitdir}/
	ln -sf ${systemd_system_unitdir}/j49-ntp-gateway.service ${D}${sysconfdir}/systemd/system/multi-user.target.wants/j49-ntp-gateway.service
	for t in network-pre.target multi-user.target; do
		install -d ${D}${sysconfdir}/systemd/system/$t.wants
		ln -sf ${systemd_system_unitdir}/j49-wlan-reprobe.service \
			${D}${sysconfdir}/systemd/system/$t.wants/j49-wlan-reprobe.service
	done
}

FILES:${PN} += "${bindir} ${systemd_unitdir}/network ${systemd_system_unitdir} ${sysconfdir}/systemd ${sysconfdir}/sysctl.d ${libexecdir}"
