// arec: minimal ALSA capture without alsa-lib (busybox has no arecord).
// Reads raw S16_LE from /dev/snd/pcmC<card>D<dev>c and writes a WAV to a file/stdout.
//   arec [-c card] [-d dev] [-r rate] [-n channels] [-s seconds] out.wav|-
// Prints the accepted rate/channels on stderr. Also prints a peak level so a
// working mic is visible without playing the file back.
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <unistd.h>
#include <sound/asound.h>

#define PERIOD 1024
#define PERIODS 4

static struct snd_mask *mask(struct snd_pcm_hw_params *p, int n) {
	return &p->masks[n - SNDRV_PCM_HW_PARAM_FIRST_MASK];
}
static struct snd_interval *iv(struct snd_pcm_hw_params *p, int n) {
	return &p->intervals[n - SNDRV_PCM_HW_PARAM_FIRST_INTERVAL];
}
static void fix(struct snd_pcm_hw_params *p, int n, unsigned v) {
	struct snd_interval *i = iv(p, n);
	i->min = i->max = v;
	i->integer = 1;
	i->openmin = i->openmax = 0;
}
static void setbit(struct snd_pcm_hw_params *p, int n, unsigned bit) {
	struct snd_mask *m = mask(p, n);
	memset(m->bits, 0, sizeof m->bits);
	m->bits[bit >> 5] = 1u << (bit & 31);
}

static int try_params(int fd, unsigned rate, unsigned ch) {
	struct snd_pcm_hw_params p;
	memset(&p, 0, sizeof p);
	for (int i = 0; i <= SNDRV_PCM_HW_PARAM_LAST_MASK - SNDRV_PCM_HW_PARAM_FIRST_MASK; i++)
		memset(p.masks[i].bits, 0xff, sizeof p.masks[i].bits);
	for (int i = 0; i <= SNDRV_PCM_HW_PARAM_LAST_INTERVAL - SNDRV_PCM_HW_PARAM_FIRST_INTERVAL; i++) {
		p.intervals[i].min = 0;
		p.intervals[i].max = UINT_MAX;
	}
	p.rmask = ~0U;
	setbit(&p, SNDRV_PCM_HW_PARAM_ACCESS, SNDRV_PCM_ACCESS_RW_INTERLEAVED);
	setbit(&p, SNDRV_PCM_HW_PARAM_FORMAT, SNDRV_PCM_FORMAT_S16_LE);
	setbit(&p, SNDRV_PCM_HW_PARAM_SUBFORMAT, SNDRV_PCM_SUBFORMAT_STD);
	fix(&p, SNDRV_PCM_HW_PARAM_CHANNELS, ch);
	fix(&p, SNDRV_PCM_HW_PARAM_RATE, rate);
	fix(&p, SNDRV_PCM_HW_PARAM_PERIOD_SIZE, PERIOD);
	fix(&p, SNDRV_PCM_HW_PARAM_PERIODS, PERIODS);
	fix(&p, SNDRV_PCM_HW_PARAM_BUFFER_SIZE, PERIOD * PERIODS);
	return ioctl(fd, SNDRV_PCM_IOCTL_HW_PARAMS, &p);
}

static void le32(uint8_t *b, uint32_t v) { for (int i = 0; i < 4; i++) b[i] = v >> (8 * i); }
static void le16(uint8_t *b, uint16_t v) { b[0] = v; b[1] = v >> 8; }

int main(int argc, char **argv) {
	int card = 1, dev = 0, secs = 3;
	unsigned rate = 48000, ch = 1;
	int o;
	while ((o = getopt(argc, argv, "c:d:r:n:s:")) != -1) {
		switch (o) {
		case 'c': card = atoi(optarg); break;
		case 'd': dev = atoi(optarg); break;
		case 'r': rate = atoi(optarg); break;
		case 'n': ch = atoi(optarg); break;
		case 's': secs = atoi(optarg); break;
		default: return 2;
		}
	}
	if (optind >= argc) { fprintf(stderr, "usage: arec [-c card] [-d dev] [-r rate] [-n ch] [-s sec] out.wav|-\n"); return 2; }
	char path[64];
	snprintf(path, sizeof path, "/dev/snd/pcmC%dD%dc", card, dev);
	int fd = open(path, O_RDONLY);
	if (fd < 0) { perror(path); return 1; }

	// card may only do certain rates/channel counts: walk common ones
	static const unsigned rates[] = {48000, 44100, 32000, 16000, 8000};
	unsigned chs[2] = {ch, ch == 1 ? 2u : 1u};
	int ok = 0;
	for (int c = 0; c < 2 && !ok; c++) {
		if (try_params(fd, rate, chs[c]) == 0) { ch = chs[c]; ok = 1; break; }
		for (unsigned r = 0; r < sizeof rates / sizeof *rates && !ok; r++)
			if (try_params(fd, rates[r], chs[c]) == 0) { rate = rates[r]; ch = chs[c]; ok = 1; }
	}
	if (!ok) { perror("HW_PARAMS (no rate/channel combo accepted)"); return 1; }
	fprintf(stderr, "arec: %s  %u Hz  %u ch  S16_LE  %d s\n", path, rate, ch, secs);
	if (ioctl(fd, SNDRV_PCM_IOCTL_PREPARE) < 0) { perror("PREPARE"); return 1; }
	if (ioctl(fd, SNDRV_PCM_IOCTL_START) < 0) { perror("START"); return 1; }

	FILE *out = strcmp(argv[optind], "-") ? fopen(argv[optind], "wb") : stdout;
	if (!out) { perror(argv[optind]); return 1; }
	uint32_t total = rate * ch * 2 * secs;
	uint8_t h[44] = "RIFF\0\0\0\0WAVEfmt \x10\0\0\0\1\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0data\0\0\0\0";
	le32(h + 4, 36 + total); le16(h + 22, ch); le32(h + 24, rate);
	le32(h + 28, rate * ch * 2); le16(h + 32, ch * 2); le16(h + 34, 16); le32(h + 40, total);
	fwrite(h, 1, 44, out);

	static int16_t buf[PERIOD * 2];
	uint32_t done = 0;
	int peak = 0;
	while (done < total) {
		ssize_t n = read(fd, buf, PERIOD * ch * 2);
		if (n < 0) {
			if (errno == EPIPE) { ioctl(fd, SNDRV_PCM_IOCTL_PREPARE); ioctl(fd, SNDRV_PCM_IOCTL_START); continue; }
			if (errno == EINTR) continue;
			perror("read");
			break;
		}
		if ((uint32_t)n > total - done) n = total - done;
		for (ssize_t i = 0; i < n / 2; i++) { int v = abs(buf[i]); if (v > peak) peak = v; }
		fwrite(buf, 1, n, out);
		done += n;
	}
	fflush(out);
	fprintf(stderr, "arec: %u bytes, peak %d / 32767\n", done, peak);
	return 0;
}
