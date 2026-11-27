/* i2cprobe: minimal /dev/i2c-N diagnostics (static, no libi2c).
 *   i2cprobe DEV scan                 -> read-byte probe 0x03..0x77, print ACKing addrs
 *   i2cprobe DEV rd ADDR REG [N]      -> N x (write REG, rd-start, read 1 byte), same
 *                                        2-message shape regmap-i2c uses; prints ok/N,
 *                                        last value and the distinct errnos seen
 * Nest J49 use: is TWL slave 0x4b (PM_MASTER/PM_RECEIVER/RTC) alive over time? */
#include <errno.h>
#include <fcntl.h>
#include <linux/i2c-dev.h>
#include <linux/i2c.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>

static int rd1(int fd, int addr, int reg, unsigned char *val)
{
	unsigned char r = reg;
	struct i2c_msg m[2] = {
		{ .addr = addr, .flags = 0, .len = 1, .buf = &r },
		{ .addr = addr, .flags = I2C_M_RD, .len = 1, .buf = val },
	};
	struct i2c_rdwr_ioctl_data d = { .msgs = m, .nmsgs = 2 };
	return ioctl(fd, I2C_RDWR, &d) < 0 ? -errno : 0;
}

int main(int argc, char **argv)
{
	if (argc < 3) { fprintf(stderr, "usage: %s DEV scan | DEV rd ADDR REG [N]\n", argv[0]); return 2; }
	int fd = open(argv[1], O_RDWR);
	if (fd < 0) { printf("%s: open: %s\n", argv[1], strerror(errno)); return 1; }
	if (!strcmp(argv[2], "scan")) {
		printf("scan %s:", argv[1]);
		for (int a = 0x03; a <= 0x77; a++) {
			unsigned char v;
			struct i2c_msg m = { .addr = a, .flags = I2C_M_RD, .len = 1, .buf = &v };
			struct i2c_rdwr_ioctl_data d = { .msgs = &m, .nmsgs = 1 };
			if (ioctl(fd, I2C_RDWR, &d) >= 0) printf(" 0x%02x", a);
		}
		printf("\n");
		return 0;
	}
	if (!strcmp(argv[2], "rd") && argc >= 5) {
		int addr = strtol(argv[3], 0, 0), reg = strtol(argv[4], 0, 0);
		int n = argc > 5 ? atoi(argv[5]) : 1, ok = 0, errs[8] = {0}, ne = 0;
		unsigned char v = 0, last = 0;
		for (int i = 0; i < n; i++) {
			int rc = rd1(fd, addr, reg, &v);
			if (rc == 0) { ok++; last = v; continue; }
			int seen = 0;
			for (int j = 0; j < ne; j++) if (errs[j] == -rc) seen = 1;
			if (!seen && ne < 8) errs[ne++] = -rc;
		}
		printf("0x%02x:0x%02x ok=%d/%d last=0x%02x", addr, reg, ok, n, last);
		for (int j = 0; j < ne; j++) printf(" err=%d(%s)", errs[j], strerror(errs[j]));
		printf("\n");
		return ok == n ? 0 : 1;
	}
	fprintf(stderr, "bad args\n");
	return 2;
}
