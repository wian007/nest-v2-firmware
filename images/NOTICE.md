# The image files of the J49 installer: who made them, licences, sources

These files are what the installer page loads onto a Nest Learning Thermostat of the 2nd
generation. They are free software. Each is listed with its makers, its licence and the place
where its source code is; the checksums are at the end. Nothing here was made by or with the
maker of the thermostat, and none of the people named is responsible for this installer.

## The loaders of the USB boot chain

| File | What | Made by | Licence | Source |
|---|---|---|---|---|
| `x-load.bin` | First loader: TI x-loader with a USB download handler. The boot ROM runs it from RAM. | Texas Instruments; the USB handler by GTVHacker (NestDFUAttack), in the variant of ajb142 | GPL-2.0 | <https://github.com/cuckoo-nest/x-loader>, <https://github.com/gtvhacker/NestDFUAttack> |
| `u-boot.bin` | U-Boot 2012.10 for this board, run from RAM only to start the service kernel. | The U-Boot project; board support from the open-source release for the thermostat; build by GTVHacker / cuckoo-nest | GPL-2.0-or-later | <https://github.com/cuckoo-nest/u-boot> |
| `nest-service-uImage` | The service kernel: Linux 2.6.37 for this board, with an initramfs of this project: `nandio` (backup, boot loader, restore), BusyBox and three libraries of the GNU C Library. | The Linux kernel developers; board support from the open-source release for the thermostat; BusyBox; GNU C Library; `nandio` by this project | Kernel GPL-2.0; BusyBox GPL-2.0; glibc LGPL-2.1-or-later; `nandio` MIT | <https://github.com/cuckoo-nest/linux>, <https://busybox.net>, <https://sourceware.org/glibc>, `tools/dfu-install/nandio` and `installer/build-service-kernel.sh` in this project |

The three are built with <https://github.com/cuckoo-nest/cuckoo_loader>, after GTVHacker's
NestDFUAttack (2014), which found that the boot ROM of the thermostat accepts code over USB.

## The system that is installed

| File | What | Licence | Source |
|---|---|---|---|
| `u-boot-nest-j49.uimg` | U-Boot 2024.07 with our board support | GPL-2.0-or-later | <https://source.denx.de/u-boot/u-boot> plus `tools/dfu-install/uboot` in this project |
| `uImage-j49-a` | Linux 6.6 with our device tree and patches | GPL-2.0 | <https://kernel.org> plus `meta-j49/recipes-kernel` in this project |
| `zImage-installer` | The same kernel with a small initramfs that writes the root file system | GPL-2.0; our programs MIT | as above, plus `tools/dfu-install/installer` |
| `*.rootfs.jffs2` | The root file system, built with the Yocto Project: many packages, each under its own licence (the build writes the list to `build/tmp/deploy/licenses`) | various free software licences | `meta-j49` and `kas/nest-j49.yml` in this project build it from the sources of each package |

This project: <https://github.com/wian007/nest-v2-firmware> (our own code MIT). To get the
source of exactly these builds, or if a file is listed wrongly, open an issue there.

The licence texts: GPL-2.0 <https://www.gnu.org/licenses/old-licenses/gpl-2.0.txt>,
LGPL-2.1 <https://www.gnu.org/licenses/old-licenses/lgpl-2.1.txt>, MIT in `LICENSE` of this
project.

## The files here

| File | Size | sha256 |
|---|---|---|
| `nest-service-uImage` | 2827820 | `94b3489173572e763dde29169c14cfa64309446e13d71432cc6bb39f071160bc` |
| `u-boot.bin` | 246572 | `52aa039f5b36d8cd5108e6bbb69f5dbd60a5c3750404c8d3fbc721d53721ccf0` |
| `x-load.bin` | 27876 | `aee635f0d09180677fa86fea6797b1ad36f64b112f7eea3a5572ab7f28fd5c21` |
