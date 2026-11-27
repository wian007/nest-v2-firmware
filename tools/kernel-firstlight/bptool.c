/*
 * bptool - talk to the Nest j49 backplate (STM32 "AMBER_BP") over UART3.
 *
 * Frame (both directions, discovered on jump 53; sync confirmed in nlclient's frame builder):
 *   d5 aa 96 | type u16 LE | len u16 LE | payload[len] | crc16 LE
 *   crc16 = CRC-16/XMODEM (poly 0x1021, init 0, no reflection) over type+len+payload.
 *   The backplate prefixes each of its frames with one extra 0xd5 (pad/wake byte), which the
 *   decoder skips. Head-unit types are 0x80.. (0xff = reset request); backplate types are small.
 *
 *   bptool sniff <tty> <seconds>                       decode incoming frames
 *   bptool send  <tty> <type-hex> [payload-hex]        send one frame, print replies for 2 s
 *   bptool beat  <tty> <type-hex> <payload-hex|-> <interval-s> <seconds>
 *                                                      send the frame every interval, decode replies
 * Static musl build: armv7l-unknown-linux-musleabihf-gcc -static -Os -o bptool bptool.c
 */
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/select.h>
#include <sys/time.h>
#include <termios.h>
#include <time.h>
#include <unistd.h>

static uint16_t crc16(const uint8_t *d, size_t n)
{
	uint16_t c = 0;
	for (size_t i = 0; i < n; i++) {
		c ^= (uint16_t)d[i] << 8;
		for (int b = 0; b < 8; b++)
			c = (c & 0x8000) ? (uint16_t)((c << 1) ^ 0x1021) : (uint16_t)(c << 1);
	}
	return c;
}

static double now(void)
{
	struct timespec ts;
	clock_gettime(CLOCK_MONOTONIC, &ts);
	return ts.tv_sec + ts.tv_nsec / 1e9;
}

static int open_tty(const char *path)
{
	int fd = open(path, O_RDWR | O_NOCTTY | O_NONBLOCK);
	if (fd < 0) { perror(path); return -1; }
	struct termios t;
	if (tcgetattr(fd, &t)) { perror("tcgetattr"); return -1; }
	cfmakeraw(&t);
	cfsetispeed(&t, B115200);
	cfsetospeed(&t, B115200);
	t.c_cflag &= ~(CRTSCTS | PARENB | CSTOPB | CSIZE);
	t.c_cflag |= CS8 | CLOCAL | CREAD;
	t.c_cc[VMIN] = 0;
	t.c_cc[VTIME] = 0;
	if (tcsetattr(fd, TCSANOW, &t)) { perror("tcsetattr"); return -1; }
	tcflush(fd, TCIOFLUSH);
	return fd;
}

static int hex2bin(const char *s, uint8_t *out, size_t max)
{
	size_t n = strlen(s);
	if (n % 2) return -1;
	for (size_t i = 0; i < n / 2 && i < max; i++) {
		unsigned v;
		if (sscanf(s + 2 * i, "%2x", &v) != 1) return -1;
		out[i] = (uint8_t)v;
	}
	return (int)(n / 2);
}

static int send_frame(int fd, uint16_t type, const uint8_t *pl, uint16_t len)
{
	uint8_t buf[3 + 4 + 1024 + 2];
	if (len > 1024) return -1;
	memcpy(buf, "\xd5\xaa\x96", 3);
	buf[3] = type & 0xff; buf[4] = type >> 8;
	buf[5] = len & 0xff;  buf[6] = len >> 8;
	memcpy(buf + 7, pl, len);
	uint16_t c = crc16(buf + 3, 4 + len);
	buf[7 + len] = c & 0xff; buf[8 + len] = c >> 8;
	size_t total = 9 + len, off = 0;
	while (off < total) {
		ssize_t w = write(fd, buf + off, total - off);
		if (w < 0) { if (errno == EAGAIN) { usleep(1000); continue; } perror("write"); return -1; }
		off += (size_t)w;
	}
	printf("[%10.3f] TX type %04x len %u:", now(), type, len);
	for (int i = 0; i < len; i++) printf(" %02x", pl[i]);
	printf("  crc %04x\n", c);
	fflush(stdout);
	return 0;
}

/* streaming decoder state */
static uint8_t rx[4096]; static size_t rxn;
static unsigned n_frames, n_bad, n_junk;

static void decode(void)
{
	size_t i = 0;
	while (rxn - i >= 9) {
		if (memcmp(rx + i, "\xd5\xaa\x96", 3)) { if (rx[i] != 0xd5) n_junk++; i++; continue; }
		uint16_t type = rx[i + 3] | rx[i + 4] << 8, len = rx[i + 5] | rx[i + 6] << 8;
		if (len > 1024) { i += 3; n_junk += 3; continue; }
		if (rxn - i < (size_t)9 + len) break;		/* wait for the rest */
		uint16_t want = rx[i + 7 + len] | rx[i + 8 + len] << 8, got = crc16(rx + i + 3, 4 + len);
		printf("[%10.3f] RX type %04x len %u:", now(), type, len);
		for (int k = 0; k < len; k++) printf(" %02x", rx[i + 7 + k]);
		if (want == got) n_frames++; else { n_bad++; printf("  CRC BAD %04x!=%04x", want, got); }
		printf("\n");
		i += 9 + len;
	}
	if (i) { memmove(rx, rx + i, rxn - i); rxn -= i; }
	fflush(stdout);
}

static void pump(int fd, double secs)
{
	double end = now() + secs;
	while (now() < end) {
		fd_set r; FD_ZERO(&r); FD_SET(fd, &r);
		struct timeval tv = { 0, 200000 };
		if (select(fd + 1, &r, NULL, NULL, &tv) > 0) {
			ssize_t n = read(fd, rx + rxn, sizeof rx - rxn);
			if (n > 0) { rxn += (size_t)n; decode(); }
			if (rxn == sizeof rx) rxn = 0;	/* hopeless garbage */
		}
	}
}

int main(int argc, char **argv)
{
	if (argc < 4) { fprintf(stderr, "usage: see header\n"); return 2; }
	int fd = open_tty(argv[2]);
	if (fd < 0) return 1;
	uint8_t pl[1024]; int len = 0;
	if (!strcmp(argv[1], "sniff")) {
		pump(fd, atof(argv[3]));
	} else if (!strcmp(argv[1], "send")) {
		if (argc > 4) len = hex2bin(argv[4], pl, sizeof pl);
		if (len < 0) { fprintf(stderr, "bad payload hex\n"); return 2; }
		send_frame(fd, (uint16_t)strtoul(argv[3], NULL, 16), pl, (uint16_t)len);
		pump(fd, 2.0);
	} else if (!strcmp(argv[1], "beat")) {
		if (argc < 7) { fprintf(stderr, "beat <tty> <type> <payload|-> <interval> <seconds>\n"); return 2; }
		if (strcmp(argv[4], "-")) len = hex2bin(argv[4], pl, sizeof pl);
		if (len < 0) { fprintf(stderr, "bad payload hex\n"); return 2; }
		uint16_t type = (uint16_t)strtoul(argv[3], NULL, 16);
		double iv = atof(argv[5]), end = now() + atof(argv[6]);
		while (now() < end) {
			send_frame(fd, type, pl, (uint16_t)len);
			pump(fd, iv);
		}
	} else { fprintf(stderr, "unknown mode %s\n", argv[1]); return 2; }
	printf("frames ok %u bad-crc %u junk-bytes %u\n", n_frames, n_bad, n_junk);
	return 0;
}
