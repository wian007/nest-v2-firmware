# Trim systemd for the 46 MiB NAND root slot (see j49-image.bb).
# vconsole pulls kbd + keymaps (1.6 MB) for a 320x320 round panel that never shows a VT;
# myhostname is a 1.6 MB NSS module for a lookup we do not need; the rest are desktop/
# container/hibernation features without a use on a 64 MB thermostat. Keep networkd,
# resolved (+ nss-resolve), timesyncd, logind, backlight, kmod.
PACKAGECONFIG:remove = "vconsole myhostname machined nss-mymachines hibernate coredump binfmt localed sysext repart"

# The ring click is the TPS65921 power button (KEY_POWER via twl4030-pwrbutton). logind
# would shut the device down on every click; the UI owns that key instead.
do_install:append() {
    install -d ${D}${sysconfdir}/systemd/logind.conf.d
    printf '[Login]\nHandlePowerKey=ignore\nHandlePowerKeyLongPress=ignore\n' \
        > ${D}${sysconfdir}/systemd/logind.conf.d/10-j49-ring-click.conf
}
FILES:${PN} += "${sysconfdir}/systemd/logind.conf.d"

# systemd recommends four kernel modules; the versioned module packages slip past
# BAD_RECOMMENDATIONS and drag kernel-image (6 MB) into the rootfs via kernel-base.
# Everything systemd wants is built in on this kernel.
RRECOMMENDS:${PN}:remove = "kernel-module-autofs4 kernel-module-unix kernel-module-ipv6 kernel-module-sch-fq-codel"
