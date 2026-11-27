# meta-j49

Yocto BSP layer for the Nest Learning Thermostat 2nd gen (J49), TI OMAP3630 /
Cortex-A8, 64 MB RAM, 256 MB NAND. Goal: a modern image (Yocto blacksail,
systemd) on the real hardware.

## Layout
- `conf/machine/nest-j49.conf` — machine definition (cortexa8hf-neon, kernel,
  console, NAND image types)
- `recipes-kernel/linux/linux-j49_6.6.bb` — mainline 6.6 LTS + the board
  patches proven by the kexec first-light work; `files/` holds the patches,
  the `j49.cfg` fragment (on `omap2plus_defconfig`) and the board DT
  `omap3-nest-j49.dts`. `tools/kernel-firstlight/build.sh` builds the same
  sources outside Yocto for quick kexec iteration.
- `recipes-bsp/u-boot/u-boot_%.bbappend` — OMAP3 bootloader (not built yet)
- `recipes-core/images/j49-image.bb` — bring-up image (core-image-minimal +
  dropbear, wpa-supplicant/iw, wl12xx firmware, kexec-tools)

## Build (NixOS + kas-container, from repo root)
The build runs inside the official Yocto container to avoid NixOS FHS issues.
Requires a running docker daemon.
```
nix develop                              # kas + kas-container + docker client
kas-container build kas/nest-j49.yml
```
`kas/nest-j49.yml` pulls bitbake 2.20 + openembedded-core + meta-yocto +
meta-openembedded on the `blacksail` branches (the poky combo repo is no
longer branched after walnascar). `LAYERSERIES_COMPAT_j49` tracks that.

Not in git (device-specific secrets): the WL1271 NVS
(`ti-connectivity/wl127x-nvs.bin`, from mtd10) and WiFi credentials.

See `../docs/modernization-plan.md` and `../docs/flash-layout.md`.
