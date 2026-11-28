# Remote kexec bring-up for the Nest J49

Hands-free way to boot a new kernel on the device **without physical access and
without flashing anything**: stage a kernel from the running stock Linux with
`kexec` and jump to it. A bad kernel is undone by a plain power-cycle (NAND still
holds stock firmware). Contrast with USB peripheral boot (tools/omap-usbboot),
which needs one physical trigger and is kept as the unbrick safety net.

Verified working on the stock `Nest-Learning-Thermostat-5.9.4-5-j49` (kernel
2.6.37, `CONFIG_KEXEC=y`).

This page is from the bring-up (October 2026): kexec from the **stock** firmware. A thermostat
with our system installed has no stock firmware to start from. On our own system, a test kernel
started with `kexec` did run, but it was unreliable (WiFi could not authenticate for minutes;
one kernel hung at its first suspend). It is not a supported way to start or update a kernel.

## 1. Build a static ARM `kexec` (no device toolchain needed)

The device is glibc 2.19, so build a **static-musl** binary that runs regardless.
Use nixpkgs cross (no zig):

```sh
nix build --no-link --print-out-paths \
  'nixpkgs#pkgsCross.armv7l-hf-multiplatform.pkgsStatic.kexec-tools'
# -> <store>/sbin/kexec : ELF 32-bit ARM, EABI5, hard-float ABI, static
```

Confirm `readelf -h` shows `hard-float ABI` (matches the device's armhf ABI).

## 2. Push to the device

`/tmp` is noexec and dropbear has no sftp-server, so copy the binary to
`/media/scratch` (jffs2) over an `ssh "cat > …"` stream, not scp:

```sh
ssh root@<host> "cat > /media/scratch/kexec && chmod +x /media/scratch/kexec" < kexec
```

Hold a stable shell first — the `nlsleepd` sleep daemon drops SSH on idle:
`ssh root@<host> "kill -STOP nlsleepd"` (restore with `kill -CONT nlsleepd`).

## 3. Stage a kernel (non-destructive) — PROVEN

For the stock kernel, strip the 64-byte u-boot uImage header off `mtd6_boot0` to
get a raw ARM zImage. The 2.6.37 kernel is non-DT (ATAG boot):

```sh
K=/media/scratch/kexec; Z=/media/scratch/stock-zImage
$K -l $Z --type=zImage --command-line="$(cat /proc/cmdline)" --atags
cat /sys/kernel/kexec_loaded      # -> 1  (staged; nothing has booted)
$K -u                             # unload
cat /sys/kernel/kexec_loaded      # -> 0
```

Observed 2026-10-26: `kexec -l` rc=0, `kexec_loaded` 0→1, `kexec -u` rc=0 → 0.
System RAM `0x80000000-0x83ffffff` (64 MB). Running cmdline:
`root=/dev/mtdblock7 rootfstype=jffs2 console= nlmodel=Display-2.14 brightness=108 panic=1`.

## 4. The live jump — only with someone present

```sh
$K -e      # jumps into the staged kernel NOW
```

A failed `kexec -e` hangs the SoC → needs a physical power-cycle. Do this step
only when someone can power-cycle, until a kernel is known-good. For a modern
kernel, stage a mainline OMAP3 (omap3.dtsi) zImage with `--dtb=<omap3-nest.dtb>`
instead of `--atags`.

## Helper

`stage.sh <host>` automates push + the non-destructive staging proof (steps 2-3)
and restores `nlsleepd`. It takes the SSH password from the environment /
`SSH_ASKPASS`; no credentials are stored in this repo.
