SUMMARY = "Linux 6.6 LTS for the Nest Learning Thermostat 2nd gen (J49, OMAP3630)"
DESCRIPTION = "Mainline kernel plus the board patches proven on the real hardware \
during the kexec first-light work (tools/kernel-firstlight): DEBUG_LL DRAM ring \
for post-mortem logs, the TWL4030/TPS65921 HFCLKIN clock kept running, \
the Samsung S6D05A1 DRM panel driver, the LM3530 backlight driver and the \
wlcore clock-xtal SDIO fix."
HOMEPAGE = "https://www.kernel.org/"
LICENSE = "GPL-2.0-only"
LIC_FILES_CHKSUM = "file://COPYING;md5=6bc538ed5bd9a7fc9398086aedcd7e46"

inherit kernel

PV = "6.6.64"

SRC_URI = "https://cdn.kernel.org/pub/linux/kernel/v6.x/linux-${PV}.tar.xz;name=kernel \
           file://0001-arm-debug-ll-dram-ring-nest-j49.patch \
           file://0003-mfd-twl-core-keep-hfclkin-enabled-nest-j49.patch \
           file://0004-drm-panel-samsung-s6d05a1-nest-j49.patch \
           file://0005-wlcore-sdio-parse-clock-xtal.patch \
           file://0006-backlight-lm3530-bl-dt-driver-nest-j49.patch \
           file://0007-arm-omap2-pdata-quirks-nest-j49-mmc2-loopback-clock.patch \
           file://0008-input-adbs-a330-nest-j49.patch \
           file://j49.cfg \
           file://omap3-nest-j49.dts \
           "
SRC_URI[kernel.sha256sum] = "065fd93fa6cb422f650fb563f15d3e0107c85009f766405993d795fd39796ab1"

S = "${UNPACKDIR}/linux-${PV}"

# certs/extract-cert, objtool-style helpers and the dtc build need these on the host.
DEPENDS += "openssl-native util-linux-native elfutils-native gmp-native libmpc-native xz-native"

COMPATIBLE_MACHINE = "nest-j49"

# Base config = omap2plus_defconfig, then the board fragment on top. The same
# pair is what tools/kernel-firstlight/build.sh builds for the kexec jumps, so a
# Yocto kernel is interchangeable with a first-light kernel.
KBUILD_DEFCONFIG = "omap2plus_defconfig"

do_configure:prepend() {
	# Board DT into the tree (idempotent).
	install -m 0644 ${UNPACKDIR}/omap3-nest-j49.dts ${S}/arch/arm/boot/dts/ti/omap/
	if ! grep -q omap3-nest-j49.dtb ${S}/arch/arm/boot/dts/ti/omap/Makefile; then
		sed -i 's#\(\s*\)omap3-beagle\.dtb \\#\1omap3-beagle.dtb \\\n\1omap3-nest-j49.dtb \\#' \
			${S}/arch/arm/boot/dts/ti/omap/Makefile
	fi

	# defconfig + fragment -> ${B}/.config; kernel_do_configure then runs olddefconfig.
	oe_runmake -C ${S} O=${B} ${KBUILD_DEFCONFIG}
	${S}/scripts/kconfig/merge_config.sh -m -O ${B} ${B}/.config ${UNPACKDIR}/j49.cfg
}
