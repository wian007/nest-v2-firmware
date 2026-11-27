// fbshim.so -- LD_PRELOAD for the stock nlclient running in a chroot on our 6.6 kernel.
// nlclient sets its own video mode (FBIOPUT_VSCREENINFO) and uses omapfb-private ioctls; the DRM
// fbdev emulation rejects them and nlclient then refuses to start. Pretend those calls succeed.
// No libc dependency except __errno_location (resolved by the stock glibc at load time).
#include <stdarg.h>
extern int *__errno_location(void);
static long raw_ioctl(int fd, unsigned long req, unsigned long arg)
{
	register long r0 __asm__("r0") = fd;
	register long r1 __asm__("r1") = (long)req;
	register long r2 __asm__("r2") = (long)arg;
	register long r7 __asm__("r7") = 54; /* __NR_ioctl */
	__asm__ volatile("svc 0" : "+r"(r0) : "r"(r1), "r"(r2), "r"(r7) : "memory");
	return r0;
}
int ioctl(int fd, unsigned long req, ...)
{
	va_list ap; va_start(ap, req);
	unsigned long arg = va_arg(ap, unsigned long);
	va_end(ap);
	long r = raw_ioctl(fd, req, arg);
	if (r < 0 && r > -4096) {
		unsigned type = (req >> 8) & 0xff;
		/* FBIOPUT_VSCREENINFO, FBIOPAN_DISPLAY, FBIOBLANK, FBIO_WAITFORVSYNC and omapfb 'O' ioctls */
		if (req == 0x4601 || req == 0x4606 || req == 0x4611 || type == 'O' ||
		    (type == 'F' && (req & 0xff) == 0x20))
			return 0;
		*__errno_location() = (int)-r;
		return -1;
	}
	return (int)r;
}

/* nlclient must never restart the device: swallow reboot() and leave a marker on flash. */
static long raw3(long nr, long a, long b, long c)
{
	register long r0 __asm__("r0") = a;
	register long r1 __asm__("r1") = b;
	register long r2 __asm__("r2") = c;
	register long r7 __asm__("r7") = nr;
	__asm__ volatile("svc 0" : "+r"(r0) : "r"(r1), "r"(r2), "r"(r7) : "memory");
	return r0;
}
int reboot(int cmd)
{
	(void)cmd;
	long fd = raw3(5 /* open */, (long)"/nestlabs/var/log/REBOOT_SYSCALL", 0x441 /* O_WRONLY|O_CREAT|O_APPEND */, 0644);
	if (fd >= 0) { raw3(4 /* write */, fd, (long)"reboot() swallowed\n", 19); raw3(118 /* fsync */, fd, 0, 0); raw3(6 /* close */, fd, 0, 0); }
	return 0;
}
