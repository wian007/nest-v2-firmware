/*
 * nandio: read and write the NAND partitions of the thermostat over a serial line.
 *
 * It talks to the installer in the browser over the USB serial gadget (/dev/ttyGS0). Reading
 * and writing go through the MTD character devices of the kernel it runs under, so the data
 * has the ECC format of that kernel. It runs in two places:
 *   - in the 2.6.37 rescue kernel that the USB boot chain starts (see ../README.md), whose
 *     format is the one the stock boot chain reads: to make a full backup before anything is
 *     written, to write our U-Boot into spl0/spl1, and to put a backup back;
 *   - in our own kernel (6.6), whose format is the one our system reads: to write the root
 *     file system into root0. There it is the first program the kernel in boot0 starts, at
 *     every start: when root0 holds a system it starts that (it mounts root0 and hands over
 *     to its /sbin/init); when it does not, it waits for the installer. So a thermostat with
 *     an empty or damaged root0 is in installer mode by itself.
 *
 * Protocol. The host sends one command per line; nandio answers with lines, and with binary
 * frames where said. Numbers are decimal unless said otherwise. Partitions are named as in
 * /proc/mtd ("spl0"), never numbered: the stock kernel and ours number them differently.
 *
 *   (start)                        nandio is the init program (/init) of the service kernel:
 *                                  nothing else runs, so nothing else reads the line. It
 *                                  sends "J49NANDIO 1" once a second until a command comes.
 *                                  (Started by hand it takes the line as its argument; "-"
 *                                  is standard input and output.)
 *   HELLO <word>                   "HELLO <word>": the host sends this with a word of its own
 *                                  until that word comes back, and throws away everything
 *                                  before it. That gets both sides in step whatever was on
 *                                  the line (kernel messages, a modem program on the PC that
 *                                  tried AT commands, an echo).
 *   IDENT                          "IDENT model=<m> hardware=<h> revision=<r>": what the board
 *                                  says it is. model is nlmodel= of the kernel command line
 *                                  (the stock boot loader passes it, "Display-2.x" on a 2nd
 *                                  generation), hardware and revision are from /proc/cpuinfo;
 *                                  "-" for what is not there, "_" for a space.
 *   INFO                           "MTD <name> <size> <erasesize>" per partition, then "OK"
 *   READ <name>                    "DATA <size> <erasesize>", then one frame per erase block:
 *                                    1 byte kind: 'D' data follows (erasesize bytes)
 *                                                 'E' the block is empty (all 0xFF), no data
 *                                                 'B' bad block, no data
 *                                                 'X' read error (ECC); data follows as read
 *                                  then "SUM <crc32 hex of the bytes sent in D and X frames>", "OK"
 *   WRITE <name> <length> <mode>   mode "shift": like nandwrite, bad blocks are skipped and the
 *                                  data moves up (a new image); mode "exact": each block goes
 *                                  where it was, bad blocks are left out (a backup of this
 *                                  device). nandio answers "READY <erasesize> <chunk>"; the
 *                                  host then sends one frame per erase block of the image:
 *                                    'D' + erasesize bytes, or 'E' (leave the block erased)
 *                                  The data of a 'D' frame goes in pieces of <chunk> bytes,
 *                                  and nandio answers every piece with "+" before the host
 *                                  sends the next: under our 6.6 kernel the USB serial gadget
 *                                  stops taking data when more comes than its small input
 *                                  buffer holds, and does not start again by itself.
 *                                  nandio answers each frame with one byte, so the host never
 *                                  runs ahead: "." done, "b" not written (mode exact, the
 *                                  block is bad). At the end "SUM <crc32 of the D data read
 *                                  back from the flash>", "OK". The host compares, leaving
 *                                  out the "b" frames.
 *   ERASE <name>                   erase the whole partition; "OK"
 *   REBOOT [dfu]                   "OK", then the board restarts. With "dfu" our U-Boot is
 *                                  told to wait for the installer instead of starting what
 *                                  is in the flash memory (a mark in RAM that survives the
 *                                  restart; see the boot policy in uboot/files/nest_j49.env).
 *                                  The answer is then "OK dfu", or "OK nomark" when the mark
 *                                  could not be written.
 *   SHOW <title> <base> <span> [text]
 *                                  what the panel of the thermostat shows: a title, a line of
 *                                  text, and a ring for the progress. The next READ, WRITE or
 *                                  ERASE moves the ring from <base> to <base>+<span> percent.
 *                                  Words are joined with "_" (shown as spaces). "OK".
 *                                  Titles DONE and ERROR colour the ring green and red.
 *   SHELL                          a root shell (busybox) on the line, for repairs by hand;
 *                                  "exit" comes back to nandio
 *   any failure                    "ERR <text>"
 *
 * Build for the device with the toolchain of the rescue kernel (build-stock-kernel.sh does it).
 * On a PC: NANDIO_FAKE=<dir> makes it use files <dir>/<name> with <dir>/mtd as the table and
 * the serial line on stdin/stdout, for tests.
 */
#define _GNU_SOURCE /* pread, pwrite */
#include <errno.h>
#include <stdarg.h>
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <termios.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#ifndef MNT_DETACH                 /* the C library of the rescue kernel's toolchain is old */
#define MNT_DETACH 2
#endif
#ifndef MS_MOVE
#define MS_MOVE 8192
#endif
#include <sys/reboot.h>
#include <sys/klog.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <sys/select.h>
#include <sys/types.h>

#define MAXPART 24
#define CHUNK 2048                 /* a piece of block data from the host; see WRITE */
#define MAXERASE (256 * 1024)

/* from <mtd/mtd-abi.h>; spelled out so the tool builds without kernel headers */
struct erase_info_user { unsigned int start, length; };
#define MEMERASE       _IOW('M', 2, struct erase_info_user)
#define MEMGETBADBLOCK _IOW('M', 11, long long)
struct mtd_ecc_stats { unsigned int corrected, failed, badblocks, bbtblocks; };
#define ECCGETSTATS    _IOR('M', 18, struct mtd_ecc_stats)

struct part { char name[32]; char dev[48]; unsigned long size, erasesize; };
static struct part parts[MAXPART];
static int nparts;
static const char *fake;      /* test directory, or NULL on the device */
static int in_fd = 0, out_fd = 1;
static unsigned char block[MAXERASE], back[MAXERASE];

/* ---- the panel: our own screens, drawn straight into the frame buffer ----
 * Nothing here may stop the work: when the frame buffer is not there or not in a format we
 * know, drawing is simply skipped. */
#include <linux/fb.h>
#include <sys/mman.h>
#include <math.h>
#include "font5x7.h"
static unsigned char *fb; static int fb_w, fb_h, fb_bpp, fb_line; static struct fb_var_screeninfo fb_var;
static char show_title[24] = "", show_text[32] = ""; static int show_base, show_span = 100, show_pct = -1;
#define C_BG 0x0a0c10
#define C_TRACK 0x171b24
#define C_ACCENT 0xf0b429
#define C_TEXT 0xd6dae3
#define C_DIM 0x6b7488
#define C_OK 0x45c16b
#define C_ERR 0xe5484d
static void fb_open(void)
{
    struct fb_fix_screeninfo fix; int fd;
    if (fake) return;
    fd = open("/dev/fb0", O_RDWR);
    if (fd < 0) return;
    if (ioctl(fd, FBIOGET_VSCREENINFO, &fb_var) || ioctl(fd, FBIOGET_FSCREENINFO, &fix)) { close(fd); return; }
    if ((fb_var.bits_per_pixel != 16 && fb_var.bits_per_pixel != 32) || fb_var.xres < 200 || fb_var.yres < 200 || fb_var.xres > 1024 || fb_var.yres > 1024) { close(fd); return; }
    fb_w = fb_var.xres; fb_h = fb_var.yres; fb_bpp = fb_var.bits_per_pixel; fb_line = fix.line_length;
    fb = mmap(NULL, (size_t)fb_line * fb_h, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0);
    if (fb == MAP_FAILED) fb = NULL;
    ioctl(fd, FBIOBLANK, 0);
    /* the file stays open: the mapping lives as long as we do */
}
static void px(int x, int y, unsigned rgb)
{
    unsigned r = (rgb >> 16) & 255, g = (rgb >> 8) & 255, b = rgb & 255, v;
    if (!fb || x < 0 || y < 0 || x >= fb_w || y >= fb_h) return;
    v = ((r >> (8 - fb_var.red.length)) << fb_var.red.offset) | ((g >> (8 - fb_var.green.length)) << fb_var.green.offset) | ((b >> (8 - fb_var.blue.length)) << fb_var.blue.offset);
    if (fb_bpp == 16) *(unsigned short *)(fb + y * fb_line + x * 2) = (unsigned short)v;
    else *(unsigned int *)(fb + y * fb_line + x * 4) = v | 0xff000000u;
}
static void text_at(int x, int y, const char *s, int scale, unsigned rgb)
{
    for (; *s; s++, x += 6 * scale) {
        unsigned i; int r, c, dx, dy; char ch = *s;
        if (ch >= 'a' && ch <= 'z') ch -= 32;
        for (i = 0; i < sizeof font5x7 / sizeof font5x7[0]; i++) if (font5x7[i].ch == ch) break;
        if (i == sizeof font5x7 / sizeof font5x7[0]) continue;
        for (r = 0; r < 7; r++) for (c = 0; c < 5; c++) if (font5x7[i].rows[r] & (0x10 >> c))
            for (dy = 0; dy < scale; dy++) for (dx = 0; dx < scale; dx++) px(x + c * scale + dx, y + r * scale + dy, rgb);
    }
}
static void text_mid(int y, const char *s, int scale, unsigned rgb)
{
    text_at(fb_w / 2 - ((int)strlen(s) * 6 * scale - scale) / 2, y, s, scale, rgb);
}
/* The whole screen: a ring from bottom-left round to bottom-right, filled up to pct. */
static void draw(int pct)
{
    int x, y, cx, cy; float r_out, r_in; unsigned ring = C_ACCENT; char num[8];
    if (!fb) return;
    if (pct < 0) pct = 0;
    if (pct > 100) pct = 100;
    if (!strcmp(show_title, "DONE")) ring = C_OK;
    if (!strcmp(show_title, "ERROR")) ring = C_ERR;
    cx = fb_w / 2; cy = fb_h / 2; r_out = (fb_w < fb_h ? fb_w : fb_h) / 2 - 8; r_in = r_out - 14;
    for (y = 0; y < fb_h; y++) for (x = 0; x < fb_w; x++) {
        float dx = x + 0.5f - cx, dy = y + 0.5f - cy, d2 = dx * dx + dy * dy; unsigned c = C_BG;
        if (d2 >= r_in * r_in && d2 <= r_out * r_out) {
            /* 0 degrees = straight down, growing clockwise; the ring runs from 45 to 315 */
            float a = atan2f(-dx, dy) * 57.29578f;
            if (a < 0) a += 360;
            if (a >= 45 && a <= 315) c = (a <= 45 + 270 * pct / 100.0f) ? ring : C_TRACK;
        }
        px(x, y, c);
    }
    text_mid(cy - 96, "J49", 3, C_ACCENT);
    text_mid(cy - 60, show_title[0] ? show_title : "INSTALLER", 2, C_TEXT);
    if (show_pct >= 0 || pct > 0) { snprintf(num, sizeof num, "%d%%", pct); text_mid(cy - 24, num, 5, C_TEXT); }
    text_mid(cy + 36, show_text, 2, C_DIM);
    show_pct = pct;
}
/* One line of small text low on the screen, without drawing the rest again: what nandio is
 * doing, for somebody who looks at the panel when the line to the PC says nothing. It also
 * goes to the kernel log. */
static void status(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void status(const char *fmt, ...)
{
    char text[40]; va_list ap; int x, y, k;
    va_start(ap, fmt); vsnprintf(text, sizeof text, fmt, ap); va_end(ap);
    if (fake) return;
    k = open("/dev/kmsg", O_WRONLY);
    if (k >= 0) { char l[64]; int n = snprintf(l, sizeof l, "nandio: %s\n", text); if (write(k, l, n) < 0) { /* no kernel log */ } close(k); }
    if (!fb) return;
    for (y = fb_h / 2 + 62; y < fb_h / 2 + 62 + 9; y++) for (x = fb_w / 2 - 110; x < fb_w / 2 + 110; x++) px(x, y, C_BG);
    text_mid(fb_h / 2 + 63, text, 1, C_DIM);
}
/* Progress inside the command that runs now: done of total. Redraws when the number changes. */
static void progress(unsigned long done, unsigned long total)
{
    int pct = show_base + (int)((double)show_span * done / (total ? total : 1));
    if (pct != show_pct) draw(pct);
}

static unsigned long crc_table[256];
static void crc_init(void)
{
    unsigned long c; int n, k;
    for (n = 0; n < 256; n++) {
        c = (unsigned long)n;
        for (k = 0; k < 8; k++) c = (c & 1) ? (0xedb88320UL ^ (c >> 1)) : (c >> 1);
        crc_table[n] = c;
    }
}
static unsigned long crc_add(unsigned long crc, const unsigned char *p, unsigned long n)
{
    crc ^= 0xffffffffUL;
    while (n--) crc = crc_table[(crc ^ *p++) & 0xff] ^ (crc >> 8);
    return (crc ^ 0xffffffffUL) & 0xffffffffUL;
}

static int put(const void *buf, size_t n)
{
    const unsigned char *p = buf;
    while (n) {
        ssize_t w = write(out_fd, p, n);
        if (w < 0) { if (errno == EINTR || errno == EAGAIN) continue; return -1; }
        p += w; n -= (size_t)w;
    }
    return 0;
}
static void say(const char *fmt, ...) __attribute__((format(printf, 1, 2)));
static void say(const char *fmt, ...)
{
    char line[256]; va_list ap; int n;
    va_start(ap, fmt); n = vsnprintf(line, sizeof line - 1, fmt, ap); va_end(ap);
    if (n < 0) return;
    if (n > (int)sizeof line - 2) n = sizeof line - 2;
    line[n++] = '\n';
    put(line, (size_t)n);
}
/* a note for somebody who looks at the panel */
static void note(const char *text)
{
    int fd;
    if (fake) return;
    fd = open("/dev/tty1", O_WRONLY | O_NOCTTY);
    if (fd >= 0) { write(fd, "[nandio] ", 9); write(fd, text, strlen(text)); write(fd, "\r\n", 2); close(fd); }
}
static int get(void *buf, size_t n)
{
    unsigned char *p = buf;
    while (n) {
        ssize_t r = read(in_fd, p, n);
        if (r == 0) return -1;
        if (r < 0) { if (errno == EINTR || errno == EAGAIN) continue; return -1; }
        p += r; n -= (size_t)r;
    }
    return 0;
}
/* One command line; with wait_ms > 0 it gives up (returns 0) when nothing comes in time. */
static int get_line(char *line, size_t max, int wait_ms)
{
    size_t n = 0;
    for (;;) {
        char c; ssize_t r;
        if (wait_ms > 0 && n == 0) {
            fd_set set; struct timeval tv;
            FD_ZERO(&set); FD_SET(in_fd, &set);
            tv.tv_sec = wait_ms / 1000; tv.tv_usec = (wait_ms % 1000) * 1000;
            if (select(in_fd + 1, &set, NULL, NULL, &tv) <= 0) return 0;
        }
        r = read(in_fd, &c, 1);
        if (r == 0) { errno = 0; return -1; }
        if (r < 0) { if (errno == EINTR || errno == EAGAIN) continue; return -1; }
        if (c == '\r') continue;
        if (c == '\n') { if (n == 0) continue; line[n] = 0; return 1; }
        if (n + 1 < max) line[n++] = c;
    }
}

static int load_parts(void)
{
    char path[256], line[160];
    FILE *f;
    if (fake) snprintf(path, sizeof path, "%s/mtd", fake); else strcpy(path, "/proc/mtd");
    f = fopen(path, "r");
    if (!f) return -1;
    nparts = 0;
    while (fgets(line, sizeof line, f) && nparts < MAXPART) {
        int idx; unsigned long size, erase; char name[64];
        /* mtd3: 00180000 00020000 "spl1" */
        if (sscanf(line, "mtd%d: %lx %lx \"%63[^\"]\"", &idx, &size, &erase, name) != 4) continue;
        if (erase == 0 || erase > MAXERASE || size % erase) continue;
        if (strlen(name) >= sizeof parts[0].name) continue;
        strcpy(parts[nparts].name, name);
        if (fake) snprintf(parts[nparts].dev, sizeof parts[0].dev, "%.30s/%.15s", fake, name);
        else snprintf(parts[nparts].dev, sizeof parts[0].dev, "/dev/mtd%d", idx);
        parts[nparts].size = size; parts[nparts].erasesize = erase;
        nparts++;
    }
    fclose(f);
    return nparts ? 0 : -1;
}
static struct part *find(const char *name)
{
    int i;
    for (i = 0; i < nparts; i++) if (!strcmp(parts[i].name, name)) return &parts[i];
    return NULL;
}
static int is_bad(int fd, unsigned long off)
{
    long long o = (long long)off;
    if (fake) {
        /* test: a block that starts with "BADBLOCK" counts as bad */
        unsigned char h[8];
        if (pread(fd, h, 8, (off_t)off) == 8 && !memcmp(h, "BADBLOCK", 8)) return 1;
        return 0;
    }
    return ioctl(fd, MEMGETBADBLOCK, &o) > 0;
}
static int erase_block(int fd, struct part *p, unsigned long off)
{
    if (fake) {
        memset(back, 0xff, p->erasesize);
        return pwrite(fd, back, p->erasesize, (off_t)off) == (ssize_t)p->erasesize ? 0 : -1;
    } else {
        struct erase_info_user e;
        e.start = (unsigned int)off; e.length = (unsigned int)p->erasesize;
        return ioctl(fd, MEMERASE, &e);
    }
}
static int all_ff(const unsigned char *b, unsigned long n)
{
    while (n--) if (*b++ != 0xff) return 0;
    return 1;
}
/* Read one erase block; 0 ok, 1 read with an error (the bytes are what the kernel gave).
 * The kernel hands the data over also when the error correction failed, without an error from
 * read(): only its counter of failed corrections tells. (Seen on the device: partitions in
 * another ECC format read "without errors" before this was counted.) */
static int read_block(int fd, struct part *p, unsigned long off, unsigned char *buf)
{
    unsigned long got = 0; int bad = 0;
    struct mtd_ecc_stats before, after;
    int stats = !fake && !ioctl(fd, ECCGETSTATS, &before);
    memset(buf, 0xff, p->erasesize);
    while (got < p->erasesize) {
        ssize_t r = pread(fd, buf + got, p->erasesize - got, (off_t)(off + got));
        if (r > 0) { got += (unsigned long)r; continue; }
        if (r < 0 && errno == EINTR) continue;
        /* an ECC error: skip one page and go on, so one bad page does not hide the rest */
        bad = 1;
        got += 2048;
    }
    if (stats && !ioctl(fd, ECCGETSTATS, &after) && after.failed != before.failed) bad = 1;
    return bad;
}

static void cmd_ident(void)
{
    char buf[1024], model[48] = "-", hw[64] = "-", rev[24] = "-", *p, *q; FILE *f; size_t n;
    if ((f = fopen(fake ? "/dev/null" : "/proc/cmdline", "r"))) {
        n = fread(buf, 1, sizeof buf - 1, f); buf[n] = 0; fclose(f);
        if ((p = strstr(buf, "nlmodel="))) sscanf(p + 8, "%47[^ \n]", model);
    }
    if ((f = fopen(fake ? "/dev/null" : "/proc/cpuinfo", "r"))) {
        while (fgets(buf, sizeof buf, f)) {
            if (!strncmp(buf, "Hardware", 8) && (p = strchr(buf, ':'))) sscanf(p + 1, " %63[^\n]", hw);
            if (!strncmp(buf, "Revision", 8) && (p = strchr(buf, ':'))) sscanf(p + 1, " %23[^\n]", rev);
        }
        fclose(f);
    }
    for (q = hw; *q; q++) if (*q == ' ') *q = '_';
    say("IDENT model=%s hardware=%s revision=%s", model, hw, rev);
}
static void cmd_info(void)
{
    int i;
    for (i = 0; i < nparts; i++) say("MTD %s %lu %lu", parts[i].name, parts[i].size, parts[i].erasesize);
    say("OK");
}
static void cmd_read(const char *name)
{
    struct part *p = find(name);
    unsigned long off, crc = 0; int fd;
    if (!p) { say("ERR no partition %s", name); return; }
    fd = open(p->dev, O_RDONLY);
    if (fd < 0) { say("ERR open %s: %s", p->name, strerror(errno)); return; }
    say("DATA %lu %lu", p->size, p->erasesize);
    note(name);
    for (off = 0; off < p->size; off += p->erasesize) {
        char kind;
        if (is_bad(fd, off)) { kind = 'B'; put(&kind, 1); continue; }
        kind = read_block(fd, p, off, block) ? 'X' : 'D';
        if (kind == 'D' && all_ff(block, p->erasesize)) { kind = 'E'; put(&kind, 1); continue; }
        if (put(&kind, 1) || put(block, p->erasesize)) { close(fd); return; }
        crc = crc_add(crc, block, p->erasesize);
        progress(off + p->erasesize, p->size);
    }
    progress(p->size, p->size);
    close(fd);
    say("SUM %08lx", crc);
    say("OK");
}
static void cmd_erase(const char *name)
{
    struct part *p = find(name);
    unsigned long off; int fd, failed = 0;
    if (!p) { say("ERR no partition %s", name); return; }
    fd = open(p->dev, O_RDWR);
    if (fd < 0) { say("ERR open %s: %s", p->name, strerror(errno)); return; }
    for (off = 0; off < p->size; off += p->erasesize)
    {
        if (!is_bad(fd, off) && erase_block(fd, p, off)) failed++;
        progress(off + p->erasesize, p->size);
    }
    close(fd);
    if (failed) say("ERR %d blocks did not erase", failed); else say("OK");
}
static void cmd_write(const char *name, unsigned long length, const char *mode)
{
    struct part *p = find(name);
    int shift = !strcmp(mode, "shift"), fd;
    unsigned long done, off = 0, crc = 0;
    const char *err = NULL;
    static char errbuf[96];
    if (!p) { say("ERR no partition %s", name); return; }
    if (!shift && strcmp(mode, "exact")) { say("ERR mode must be shift or exact"); return; }
    if (length == 0 || length % p->erasesize || length > p->size) { say("ERR length must be whole erase blocks and fit"); return; }
    fd = open(p->dev, O_RDWR);
    if (fd < 0) { say("ERR open %s: %s", p->name, strerror(errno)); return; }
    say("READY %lu %d", p->erasesize, CHUNK);
    note(name);
    /* From here every frame of the image is taken, also after an error: the host must not be
     * left writing into a line nobody reads. */
    for (done = 0; done < length; done += p->erasesize) {
        char kind, ans = '.';
        unsigned long nblk = done / p->erasesize + 1, total = length / p->erasesize;
        /* the first blocks say on the panel where they are: when a write stands still, that
         * shows in which part */
        if (nblk <= 3) status("%s %lu/%lu RECEIVE", p->name, nblk, total);
        if (get(&kind, 1)) { close(fd); return; }
        if (kind == 'D') {
            unsigned long at;
            for (at = 0; at < p->erasesize; at += CHUNK) {
                unsigned long n = p->erasesize - at < CHUNK ? p->erasesize - at : CHUNK;
                if (get(block + at, n)) { close(fd); return; }
                put("+", 1);
            }
        }
        else if (kind != 'E') { if (!err) err = "bad frame"; kind = 'E'; }
        if (nblk <= 3) status("%s %lu/%lu WRITE", p->name, nblk, total);
        if (!err) {
            if (shift) {
                while (off < p->size && is_bad(fd, off)) off += p->erasesize;
            } else {
                off = done;
            }
            if (off >= p->size) {
                err = "no room: too many bad blocks";
            } else if (!shift && is_bad(fd, off)) {
                ans = 'b'; /* the backup had nothing useful here either */
            } else if (erase_block(fd, p, off)) {
                snprintf(errbuf, sizeof errbuf, "erase at %lu: %s", off, strerror(errno)); err = errbuf;
            } else if (kind == 'D') {
                if (pwrite(fd, block, p->erasesize, (off_t)off) != (ssize_t)p->erasesize) {
                    snprintf(errbuf, sizeof errbuf, "write at %lu: %s", off, strerror(errno)); err = errbuf;
                } else if (read_block(fd, p, off, back) || memcmp(block, back, p->erasesize)) {
                    snprintf(errbuf, sizeof errbuf, "verify at %lu", off); err = errbuf;
                } else {
                    crc = crc_add(crc, back, p->erasesize);
                }
            }
            if (shift) off += p->erasesize;
        }
        put(&ans, 1);
        if (nblk <= 3 || nblk % 8 == 0) status("%s %lu/%lu", p->name, nblk, total);
        progress(done + p->erasesize, length);
    }
    /* shift: what is left of the partition must not keep an old image */
    if (!err && shift)
        for (; off < p->size; off += p->erasesize)
            if (!is_bad(fd, off)) erase_block(fd, p, off);
    if (!fake) fsync(fd);
    close(fd);
    if (err) say("ERR %s", err);
    else { say("SUM %08lx", crc); say("OK"); }
}

static struct termios cooked;
/* Restart the board, and make sure of it. Which way works depends on the kernel:
 *  - the 2.6.37 rescue kernel restarts with reboot();
 *  - under our 6.6 kernel reboot() leaves the board dead until the ring is held, and the
 *    watchdog is the clean way (it is what the installer always used).
 * After the way that fits, the others are tried too: the watchdog left to run out, and the
 * software reset bits of the OMAP3 reset controller. */
#include <sys/utsname.h>
static void restart(void)
{
    struct utsname u; int fd, old = 0;
    if (!uname(&u) && !strncmp(u.release, "2.6.", 4)) old = 1;
    sync(); sleep(1);
    note("restart");
    if (old) { reboot(RB_AUTOBOOT); sleep(3); }
    fd = open("/dev/watchdog", O_WRONLY);
    if (fd >= 0) { int t = 2; ioctl(fd, _IOWR('W', 6, int), &t); /* WDIOC_SETTIMEOUT; never fed */ sleep(8); }
    fd = open("/dev/mem", O_RDWR | O_SYNC);
    if (fd >= 0) {
        /* PRM_RSTCTRL at 0x48307250: bit 1 = global software reset, bit 2 = reset with DPLL3 */
        volatile unsigned int *prm = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0x48307000);
        if (prm != MAP_FAILED) { prm[0x250 / 4] |= 2; sleep(2); prm[0x250 / 4] |= 4; sleep(2); }
        close(fd);
    }
    if (!old) reboot(RB_AUTOBOOT);
    for (;;) sleep(60);
}
/* As init: what a system normally sets up before a program runs. */
static void init_system_early(void)
{
    mkdir("/proc", 0555); mkdir("/sys", 0555); mkdir("/dev", 0755); mkdir("/tmp", 01777);
    mount("proc", "/proc", "proc", 0, NULL);
    mount("sysfs", "/sys", "sysfs", 0, NULL);
    /* device nodes from the kernel, with the numbers it really uses; the nodes that are in
     * the initramfs stay when this fails */
    mount("devtmpfs", "/dev", "devtmpfs", 0, NULL);
}
static void init_system(void)
{
    int fd;
    /* in case boot_system() took them away on its way out */
    mount("proc", "/proc", "proc", 0, NULL);
    mount("sysfs", "/sys", "sysfs", 0, NULL);
    /* under our kernel the text console would paint over our screen: take it off the panel */
    fd = open("/sys/class/vtconsole/vtcon1/bind", O_WRONLY);
    if (fd >= 0) { if (write(fd, "0", 1) < 0) { /* no text console there */ } close(fd); }
    /* the kernel's console is this same serial line: its messages must not get into the data */
    klogctl(6, NULL, 0);
}
/* Under our own kernel, as init: start the system in root0 when there is one. Returns only
 * when there is none (or it cannot be started): the caller then serves the installer.
 * "A system" = root0 mounts as jffs2 and has an init program. */
static void boot_system(void)
{
    static const char *const inits[] = { "/sbin/init", "/lib/systemd/systemd", "/usr/lib/systemd/systemd", NULL };
    char dev[32] = ""; struct utsname u; int i, tries;
    if (uname(&u) || !strncmp(u.release, "2.6.", 4)) return;      /* the rescue kernel: never */
    if (getenv("NANDIO_INSTALL")) return;                          /* asked for on the kernel command line */
    /* which partition: the last root=/dev/mtdblockN of the kernel command line (the kernel for
     * the second slot names root1 there), else the one called root0 */
    {
        char cmd[1024]; FILE *f = fopen("/proc/cmdline", "r"); size_t n; char *p, *last = NULL;
        if (f) {
            n = fread(cmd, 1, sizeof cmd - 1, f); cmd[n] = 0; fclose(f);
            for (p = cmd; (p = strstr(p, "root=/dev/mtdblock")); p += 5) last = p + 5;
            if (last) sscanf(last, "%31[^ \n]", dev);
        }
    }
    if (!dev[0]) {
        if (load_parts()) return;
        for (i = 0; i < nparts; i++)
            if (!strcmp(parts[i].name, "root0")) snprintf(dev, sizeof dev, "/dev/mtdblock%s", parts[i].dev + strlen("/dev/mtd"));
    }
    if (!dev[0]) return;
    mkdir("/newroot", 0755);
    /* the block device shows up a moment after the kernel has scanned the flash memory */
    for (tries = 0; tries < 50 && access(dev, F_OK); tries++) usleep(100000);
    /* Look before mounting: an empty partition is not mounted at all. Mounted read-write on
     * empty flash, jffs2 starts to write by itself; on this board that code has crashed in the
     * kernel before, and the flash memory then stays locked: the installer's first write
     * never came back (27 November 2026). So: no jffs2 start mark, no mount. */
    {
        char chr[32]; unsigned char head[2] = { 0xff, 0xff }; unsigned long off, erase = 0x20000, size = 0; int fd, found = 0;
        snprintf(chr, sizeof chr, "/dev/mtd%s", dev + strlen("/dev/mtdblock"));
        if (!load_parts()) for (i = 0; i < nparts; i++) if (!strcmp(parts[i].dev, chr)) { erase = parts[i].erasesize; size = parts[i].size; }
        fd = open(chr, O_RDONLY);
        if (fd < 0) return;
        /* the mark (0x1985, either byte order) at the start of one of the first good blocks */
        for (off = 0; off < size && off < 8 * erase && !found; off += erase) {
            long long o = (long long)off;
            if (ioctl(fd, MEMGETBADBLOCK, &o) > 0) continue;
            if (pread(fd, head, 2, (off_t)off) == 2 && ((head[0] == 0x85 && head[1] == 0x19) || (head[0] == 0x19 && head[1] == 0x85))) found = 1;
        }
        close(fd);
        if (!found) return;
    }
    /* read-only, as the kernel itself mounts a root file system; the system remounts it */
    if (mount(dev, "/newroot", "jffs2", MS_RDONLY, NULL)) return;
    for (i = 0; inits[i]; i++) {
        char path[80]; snprintf(path, sizeof path, "/newroot%s", inits[i]);
        if (!access(path, X_OK)) break;
    }
    if (!inits[i]) { umount("/newroot"); return; }                  /* empty flash mounts too: no system in it */
    /* hand over: the new root becomes "/", ours is left behind (it is a few files in RAM) */
    umount2("/proc", MNT_DETACH); umount2("/sys", MNT_DETACH);
    mount("/dev", "/newroot/dev", NULL, MS_MOVE, NULL);
    if (chdir("/newroot") || mount(".", "/", NULL, MS_MOVE, NULL) || chroot(".") || chdir("/")) return;
    { int fd = open("/dev/console", O_RDWR); if (fd >= 0) { dup2(fd, 0); dup2(fd, 1); dup2(fd, 2); if (fd > 2) close(fd); } }
    execl(inits[i], inits[i], (char *)NULL);
    /* not reached when it worked; when it did not, there is nothing sane to fall back to */
    for (;;) sleep(60);
}
int main(int argc, char **argv)
{
    char line[160];
    const char *tty = argc > 1 ? argv[1] : "/dev/ttyGS0";
    int started = 0, is_init = getpid() == 1;
    fake = getenv("NANDIO_FAKE");
    crc_init();
    if (is_init && !fake) {
        init_system_early();
        boot_system();              /* comes back only when there is no system to start */
        init_system();
    }
    if (!fake) {
        struct termios t;
        /* "-": the line we were started on, else a device to open. As init: keep trying,
         * the USB gadget may not be there yet, and init must never end. */
        int fd = strcmp(tty, "-") ? open(tty, O_RDWR | O_NOCTTY) : 0;
        while (fd < 0 && is_init) { sleep(1); fd = open(tty, O_RDWR | O_NOCTTY); }
        if (fd < 0) { perror(tty); return 1; }
        tcgetattr(fd, &cooked);
        /* raw: the line carries binary data in both directions */
        if (!tcgetattr(fd, &t)) { cfmakeraw(&t); t.c_cc[VMIN] = 1; t.c_cc[VTIME] = 0; tcsetattr(fd, TCSANOW, &t); }
        /* A line we opened ourselves is read and written through the same descriptor. Only
         * with "-" (fd 0) is the output descriptor 1. This used to be decided by "fd is 0":
         * as init under our kernel nothing is open yet, the line itself becomes descriptor 0,
         * and everything nandio said went to a descriptor that did not exist. (Found on the
         * device, 27 November 2026: the screen said CONNECTED, the PC received nothing.) */
        in_fd = fd; out_fd = strcmp(tty, "-") ? fd : 1;
    }
    /* nandio starts early in the boot of the rescue system: /proc is not mounted yet then
     * (seen on the device: "no partition table"). Mounting it twice does no harm. */
    if (!fake && !is_init) { mkdir("/proc", 0555); mount("proc", "/proc", "proc", 0, NULL); }
    fb_open();
    draw(0);
    status("LINE OPEN");
    while (load_parts()) {
        say("ERR no partition table: %s", strerror(errno));
        status("NO PARTITION TABLE");
        if (!is_init) return 1;
        sleep(2);
    }
    status("%d PARTITIONS", nparts);
    {
    unsigned long banners = 0, gone = 0;
    for (;;) {
        char cmd[16], a[64], c[16]; unsigned long n = 0; int k;
        int r = get_line(line, sizeof line, started ? 0 : 1000);
        if (r < 0) {
            /* Nothing can be read: the cable is out, the PC has not opened the port yet, or it
             * reset the port. That is not an end: wait, open the line again, go on. (Seen on
             * the device: after a reconnect nandio was deaf; and under our kernel it said
             * nothing at all, which is why the screen now says what it is doing.) */
            int e = errno;
            if (!is_init) return 0;
            gone++;
            status("NO LINE %lu E%d", gone, e);
            sleep(1);
            if (strcmp(tty, "-")) {
                struct termios t; int fd = open(tty, O_RDWR | O_NOCTTY);
                if (fd >= 0) {
                    close(in_fd);
                    if (!tcgetattr(fd, &t)) { cooked = t; cfmakeraw(&t); t.c_cc[VMIN] = 1; t.c_cc[VTIME] = 0; tcsetattr(fd, TCSANOW, &t); }
                    in_fd = out_fd = fd;
                }
            }
            started = 0;
            say("J49NANDIO 1");
            continue;
        }
        if (r == 0) { say("J49NANDIO 1"); if (++banners % 5 == 1) status("WAITING %lu", banners); continue; }
        if (!started) status("CONNECTED");
        started = 1;
        cmd[0] = a[0] = c[0] = 0;
        k = sscanf(line, "%15s %63s %lu %15s", cmd, a, &n, c);
        if (k < 1) continue;
        /* Our own lines coming back are an echo (a terminal on the other side that is not in
         * raw mode yet): answering them would go round for ever. */
        if (!strcmp(cmd, "J49NANDIO") || !strcmp(cmd, "ERR") || !strcmp(cmd, "OK") || !strcmp(cmd, "MTD")) { started = 0; continue; }
        if (!strcmp(cmd, "SHOW") && k >= 2) {
            /* SHOW <title> <base> <span> [text]: sscanf above took title, base and (as a word) span */
            char t[32] = "", x[32] = ""; int b = 0, sp = 100; char *q;
            sscanf(line, "%*s %23s %d %d %31s", t, &b, &sp, x);
            for (q = t; *q; q++) if (*q == '_') *q = ' ';
            for (q = x; *q; q++) if (*q == '_') *q = ' ';
            strcpy(show_title, t); strcpy(show_text, x);
            show_base = b < 0 ? 0 : b > 100 ? 100 : b; show_span = sp < 0 ? 0 : sp > 100 - show_base ? 100 - show_base : sp;
            show_pct = -1; draw(show_base);
            say("OK");
        }
        else if (!strcmp(cmd, "HELLO") && k >= 2) say("HELLO %s", a);
        else if (!strcmp(cmd, "IDENT")) cmd_ident();
        else if (!strcmp(cmd, "INFO")) cmd_info();
        else if (!strcmp(cmd, "READ") && k >= 2) cmd_read(a);
        else if (!strcmp(cmd, "ERASE") && k >= 2) cmd_erase(a);
        else if (!strcmp(cmd, "WRITE") && k == 4) cmd_write(a, n, c);
        else if (!strcmp(cmd, "REBOOT")) {
            int marked = 0;
            if (k >= 2 && !strcmp(a, "dfu") && !fake) {
                /* 0xdf00df00 at 0x83e00000: "stay in the DFU window", read once by our U-Boot */
                int fd = open("/dev/mem", O_RDWR | O_SYNC);
                if (fd >= 0) {
                    volatile unsigned int *m = mmap(NULL, 4096, PROT_READ | PROT_WRITE, MAP_SHARED, fd, 0x83e00000);
                    if (m != MAP_FAILED) { m[0] = 0xdf00df00u; marked = m[0] == 0xdf00df00u; munmap((void *)m, 4096); }
                    close(fd);
                }
                say(marked ? "OK dfu" : "OK nomark");
            } else
            say("OK");
            note("restart");
            if (fake) return 0;
            restart();
        } else if (!strcmp(cmd, "SHELL") && !fake) {
            pid_t pid;
            say("OK");
            tcsetattr(in_fd, TCSANOW, &cooked);
            pid = fork();
            if (pid == 0) {
                setsid(); ioctl(in_fd, TIOCSCTTY, 0);
                dup2(in_fd, 0); dup2(out_fd, 1); dup2(out_fd, 2);
                execl("/bin/sh", "sh", (char *)NULL);
                _exit(127);
            }
            if (pid > 0) { int st; while (waitpid(pid, &st, 0) < 0 && errno == EINTR) {} }
            { struct termios t = cooked; cfmakeraw(&t); t.c_cc[VMIN] = 1; t.c_cc[VTIME] = 0; tcsetattr(in_fd, TCSANOW, &t); }
            started = 0;
        } else say("ERR unknown command");
    }
    }
}
