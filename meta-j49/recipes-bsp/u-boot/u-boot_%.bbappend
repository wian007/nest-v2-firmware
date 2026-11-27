# meta-j49 u-boot customisation for Nest J49 (OMAP3)
#
# Phase 1: build mainline u-boot using a known OMAP3 config (omap3_beagle)
# to validate SPL(MLO)+u-boot.img on the J49 over serial.
# Later: add a nest_j49_defconfig + board env (NAND partitions, A/B boot).

FILESEXTRAPATHS:prepend := "${THISDIR}/files:"

COMPATIBLE_MACHINE:nest-j49 = "nest-j49"

# SRC_URI:append:nest-j49 = " file://nest-j49-env.txt"
