/*
 * fbanim - a self-contained framebuffer toy for the Nest j49 round 320x320 panel.
 *
 * Draws a rotating conic rainbow masked to the inscribed circle (radius 160), with a
 * progress arc from the top that fills over DEADLINE seconds. While the backplate's
 * ~29 min head-unit watchdog is the thing under test, the arc fills toward 29:00; when
 * it completes it turns green and a second hand keeps sweeping, so a glance says
 * "still alive past the deadline" = keepalive works.
 *
 * It owns /dev/fb0 directly (xRGB8888, 320x320). Run after unbinding fbcon:
 *   echo 0 > /sys/class/vtconsole/vtcon1/bind
 *   fbanim &
 * Static musl build: armv7l-unknown-linux-musleabihf-gcc -static -O2 -o fbanim fbanim.c -lm
 */
#include <fcntl.h>
#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>
#include <sys/mman.h>
#include <time.h>
#include <unistd.h>

#define W 320
#define H 320
#define CX 159.5
#define CY 159.5
#define R 160.0
#define DEADLINE 1740.0		/* 29:00, the backplate watchdog window */

static uint32_t fb[W * H];

static uint32_t hsv(double h, double s, double v)
{
	h = fmod(h, 360.0); if (h < 0) h += 360.0;
	double c = v * s, x = c * (1 - fabs(fmod(h / 60.0, 2) - 1)), m = v - c, r, g, b;
	if (h < 60)      { r = c; g = x; b = 0; }
	else if (h < 120){ r = x; g = c; b = 0; }
	else if (h < 180){ r = 0; g = c; b = x; }
	else if (h < 240){ r = 0; g = x; b = c; }
	else if (h < 300){ r = x; g = 0; b = c; }
	else             { r = c; g = 0; b = x; }
	uint32_t R8 = (uint32_t)((r + m) * 255), G8 = (uint32_t)((g + m) * 255), B8 = (uint32_t)((b + m) * 255);
	return (R8 << 16) | (G8 << 8) | B8;
}

static double uptime(void)
{
	double u = 0; FILE *f = fopen("/proc/uptime", "r");
	if (f) { if (fscanf(f, "%lf", &u) != 1) u = 0; fclose(f); }
	return u;
}

int main(void)
{
	int fd = open("/dev/fb0", O_RDWR);
	if (fd < 0) { perror("/dev/fb0"); return 1; }
	uint32_t *mem = mmap(NULL, sizeof fb, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
	int mapped = mem != MAP_FAILED;
	double t0 = uptime();

	for (long frame = 0;; frame++) {
		double spin = frame * 2.2;		/* rainbow rotation, deg/frame */
		double up = uptime();
		double since = up - t0;
		double frac = up / DEADLINE; if (frac > 1) frac = 1;
		int past = up >= DEADLINE;
		double hand = fmod(since, 6.0) / 6.0 * 360.0;	/* 6 s sweep second-hand */

		for (int y = 0; y < H; y++) {
			for (int x = 0; x < W; x++) {
				double dx = x - CX, dy = y - CY, d = sqrt(dx * dx + dy * dy);
				uint32_t px;
				if (d > R) { px = 0x000000; }		/* corner, hidden by bezel */
				else {
					double ang = atan2(dy, dx) * 180.0 / M_PI;	/* -180..180 */
					double v = 0.35 + 0.65 * (d / R);		/* dim centre -> bright rim */
					px = hsv(ang + spin, 1.0, v);
					/* progress ring at r 150..158 */
					if (d >= 150 && d <= 158) {
						double a = ang + 90.0; if (a < 0) a += 360.0;	/* 0 at top, clockwise */
						if (a <= frac * 360.0)
							px = past ? 0x00ff66 : 0xffffff;
						else
							px = 0x202020;
					}
					/* sweeping second-hand: a bright radial spoke */
					double a2 = ang + 90.0; if (a2 < 0) a2 += 360.0;
					double da = fabs(a2 - hand); if (da > 180) da = 360 - da;
					if (d < 150 && da < 2.2)
						px = past ? 0xffffff : 0x00e0ff;
					/* centre hub */
					if (d < 10) px = past ? 0x00ff66 : 0xffffff;
				}
				fb[y * W + x] = px;
			}
		}
		if (mapped) memcpy(mem, fb, sizeof fb);
		else { lseek(fd, 0, SEEK_SET); if (write(fd, fb, sizeof fb) < 0) {} }

		if ((frame % 40) == 0)
			printf("fbanim up=%.0f frac=%.2f past29=%d\n", up, frac, past), fflush(stdout);
		usleep(80000);		/* ~12 fps */
	}
}
