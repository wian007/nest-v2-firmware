SUMMARY = "Minimal modern image for the Nest J49 (bring-up)"
LICENSE = "MIT"

require recipes-core/images/core-image-minimal.bb

# 64 MB RAM, 256 MB NAND: keep it small. Dropbear instead of OpenSSH (the
# first-light sessions already run a static dropbear on the device).
# bring-up only: root without password, autologin on the console (was "debug-tweaks")
IMAGE_FEATURES += "allow-empty-password allow-root-login empty-root-password serial-autologin-root ssh-server-dropbear"

IMAGE_INSTALL:append = " \
    linux-firmware-wl12xx \
    wireless-regdb-static \
    wpa-supplicant \
    iw \
    i2c-tools \
    mtd-utils \
    util-linux-fsck \
    bpd \
    j49-screen \
    j49-mqtt \
    j49-web \
    j49-sleep \
    j49-wifi \
    j49-update \
    j49-dfu \
    j49-config \
"

# kexec-tools (+kdump, makedumpfile, vmcore-dmesg) was here for in-image iteration; the kexec
# jumps are driven from stock, and kdump.service at boot is dead weight on 64 MB.
# Size budget: the NAND root slot is 46 MiB of jffs2 (docs/flash-layout.md). The first
# build was 56 MB; these recommendations alone were ~25 MB of it (udev-hwdb 9 MB,
# systemd-mime -> shared-mime-info -> glib/libxml2/libstdc++ 12 MB). kexec-tools hard-RDEPENDS on
# makedumpfile (380 KB), so that one stays.
BAD_RECOMMENDATIONS += " \
    udev-hwdb \
    systemd-mime \
    systemd-crypt \
    systemd-extra-utils \
    e2fsprogs-e2fsck \
    kernel-module-autofs4 kernel-module-unix kernel-module-ipv6 kernel-module-sch-fq-codel \
"

# tar for inspection, jffs2 for the NAND B slot, squashfs-xz for the RAM-resident kexec test
# (tools/kernel-firstlight/squash-init loop-mounts it under a tmpfs overlay). The cpio.gz of
# the whole rootfs (45 MB unpacked) does not fit next to the kernel in 64 MB, hence squashfs.
IMAGE_FSTYPES ?= "tar.xz jffs2 squashfs-xz"

# The DFU installer stages the jffs2 in a fixed U-Boot window (dfu_alt_info "rootfs" = 0x01510000 =
# 22085632 bytes, tools/dfu-install/uboot/files/nest_j49.env). A longer image overruns it and the
# download hangs the device. Keep the image below that, and check it (do_image_jffs2 output).
# Dead weight on a fixed read-only root: ldconfig (ld.so.cache is built at image time) and the
# multi-role WL127x firmware (STA-only uses wl127x-fw-5-sr.bin).
ROOTFS_POSTPROCESS_COMMAND += "nest_trim_rootfs;"
nest_trim_rootfs() {
    rm -f ${IMAGE_ROOTFS}${sbindir}/ldconfig
    rm -f ${IMAGE_ROOTFS}${nonarch_base_libdir}/firmware/ti-connectivity/wl127x-fw-5-mr.bin
}

# /etc/j49-release: which commit of this repository the image was built from, and when.
#   commit=<full hash>  dirty=<0|1>  commit_date=<ISO date of that commit>  built=<UTC build time>
# The git queries run at parse time against the layer's repository; "unknown" when git cannot
# read it (the kas container may see the checkout with another owner, hence safe.directory).
# The installer artifacts under tools/dfu-install/installer/out are built FROM this image, so
# they are left out of the dirty check.
def j49_git(d, args):
    import subprocess
    try:
        return subprocess.check_output(
            ['git', '-c', 'safe.directory=*', '-C', d.getVar('LAYERDIR_j49')] + args,
            stderr=subprocess.DEVNULL).decode().strip()
    except Exception:
        return 'unknown'

J49_COMMIT := "${@j49_git(d, ['rev-parse', 'HEAD'])}"
J49_COMMIT_DATE := "${@j49_git(d, ['log', '-1', '--format=%cI'])}"
J49_DIRTY := "${@'1' if j49_git(d, ['status', '--porcelain', '--untracked-files=no', '--', ':(top)', ':(top,exclude)tools/dfu-install/installer/out']) not in ('', 'unknown') else '0'}"

ROOTFS_POSTPROCESS_COMMAND += "j49_release;"
j49_release() {
    {
        echo "commit=${J49_COMMIT}"
        echo "dirty=${J49_DIRTY}"
        echo "commit_date=${J49_COMMIT_DATE}"
        echo "built=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    } > ${IMAGE_ROOTFS}${sysconfdir}/j49-release
}
