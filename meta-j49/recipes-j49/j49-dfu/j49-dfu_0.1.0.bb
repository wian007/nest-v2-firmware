SUMMARY = "Reboot the Nest into the U-Boot DFU window or USB-ACM console from Linux"
DESCRIPTION = "Writes a one-shot flag word to DRAM 0x83e00000 (mmap on /dev/mem) and reboots. \
Our U-Boot (tools/dfu-install/uboot, files/nest_j49.env) reads it in bootcmd, clears it, and \
stays in the DFU window (0xdf00df00, dfu-util 0451:df10) or opens the console (0xac0ac0ac) \
instead of booting boot0. DRAM survives the warm reset."
HOMEPAGE = "https://github.com/wian007/nest-v2-firmware"
LICENSE = "MIT"
LIC_FILES_CHKSUM = "file://${COMMON_LICENSE_DIR}/MIT;md5=0835ade698e0bcf8506ecda2f7b4f302"

inherit cargo

# Source lives in tools/j49-dfu (no crate dependencies, so nothing to vendor).
FILESEXTRAPATHS:prepend := "${THISDIR}/../../../tools:"
SRC_URI = "file://j49-dfu/Cargo.toml \
           file://j49-dfu/Cargo.lock \
           file://j49-dfu/src/main.rs \
           "

S = "${UNPACKDIR}/j49-dfu"

COMPATIBLE_MACHINE = "nest-j49"
