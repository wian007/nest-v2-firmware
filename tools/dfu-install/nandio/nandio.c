/*
 * nandio: read and write the NAND partitions of the thermostat over a serial line.
 *
 * It runs in the stock 2.6.37 rescue kernel that the USB boot chain starts (see ../README.md)
 * and talks to the installer in the browser over the USB serial gadget (/dev/ttyGS0). Reading
 * and writing go through the kernel's MTD character devices, so the data has the ECC format the
 * stock boot chain reads. With it the installer can
 *   - make a full backup before anything is written,
 *   - write our U-Boot into spl0/spl1 (the first stage of the install),
 *   - put a backup back.
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
 *                                  device). nandio answers "READY <erasesize>"; the host then
 *                                  sends one frame per erase block of the image:
 *                                    'D' + erasesize bytes, or 'E' (leave the block erased)
 *                                  nandio answers each frame with one byte, so the host never
 *                                  runs ahead: "." done, "b" not written (mode exact, the
 *                                  block is bad). At the end "SUM <crc32 of the D data read
 *                                  back from the flash>", "OK". The host compares, leaving
 *                                  out the "b" frames.
 *   ERASE <name>                   erase the whole partition; "OK"
 *   REBOOT                         "OK", then the board restarts
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
#include <fcntl.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <termios.h>
#include <unistd.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/reboot.h>
#include <sys/klog.h>
#include <sys/wait.h>
#include <sys/stat.h>
#include <sys/select.h>
#include <sys/types.h>

#define MAXPART 24
#define MAXERASE (256 * 1024)

/* from <mtd/mtd-abi.h>; spelled out so the tool builds without kernel headers */
struct erase_info_user { unsigned int start, length; };
#define MEMERASE       _IOW('M', 2, struct erase_info_user)
#define MEMGETBADBLOCK _IOW('M', 11, long long)

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
#include <stdarg.h>
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
        if (r == 0) return -1;
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
/* Read one erase block; 0 ok, 1 read with an error (the bytes are what the kernel gave). */
static int read_block(int fd, struct part *p, unsigned long off, unsigned char *buf)
{
    unsigned long got = 0; int bad = 0;
    memset(buf, 0xff, p->erasesize);
    while (got < p->erasesize) {
        ssize_t r = pread(fd, buf + got, p->erasesize - got, (off_t)(off + got));
        if (r > 0) { got += (unsigned long)r; continue; }
        if (r < 0 && errno == EINTR) continue;
        /* an ECC error: skip one page and go on, so one bad page does not hide the rest */
        bad = 1;
        got += 2048;
    }
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
    say("READY %lu", p->erasesize);
    note(name);
    /* From here every frame of the image is taken, also after an error: the host must not be
     * left writing into a line nobody reads. */
    for (done = 0; done < length; done += p->erasesize) {
        char kind, ans = '.';
        if (get(&kind, 1)) { close(fd); return; }
        if (kind == 'D') { if (get(block, p->erasesize)) { close(fd); return; } }
        else if (kind != 'E') { if (!err) err = "bad frame"; kind = 'E'; }
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
static void restart(void)
{
    sync(); sleep(1);
    reboot(RB_AUTOBOOT);
    for (;;) sleep(60);
}
/* As init: what a system normally sets up before a program runs. */
static void init_system(void)
{
    mkdir("/proc", 0555); mkdir("/sys", 0555); mkdir("/dev", 0755); mkdir("/tmp", 01777);
    mount("proc", "/proc", "proc", 0, NULL);
    mount("sysfs", "/sys", "sysfs", 0, NULL);
    /* device nodes from the kernel, with the numbers it really uses; the nodes that are in
     * the initramfs stay when this fails */
    mount("devtmpfs", "/dev", "devtmpfs", 0, NULL);
    /* the kernel's console is this same serial line: its messages must not get into the data */
    klogctl(6, NULL, 0);
}
int main(int argc, char **argv)
{
    char line[160];
    const char *tty = argc > 1 ? argv[1] : "/dev/ttyGS0";
    int started = 0, is_init = getpid() == 1;
    fake = getenv("NANDIO_FAKE");
    crc_init();
    if (is_init && !fake) init_system();
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
        in_fd = fd; out_fd = fd ? fd : 1;
    }
    /* nandio starts early in the boot of the rescue system: /proc is not mounted yet then
     * (seen on the device: "no partition table"). Mounting it twice does no harm. */
    if (!fake && !is_init) { mkdir("/proc", 0555); mount("proc", "/proc", "proc", 0, NULL); }
    fb_open();
    draw(0);
    while (load_parts()) {
        say("ERR no partition table: %s", strerror(errno));
        if (!is_init) return 1;
        sleep(2);
    }
    note("ready: waiting for the installer on USB");
    for (;;) {
        char cmd[16], a[64], c[16]; unsigned long n = 0; int k;
        int r = get_line(line, sizeof line, started ? 0 : 1000);
        if (r < 0) {
            /* The line is gone: the cable was pulled or the PC reset the port. As init, open
             * it again and start over (seen on the device: after a reconnect nandio was deaf). */
            if (!is_init) return 0;
            if (strcmp(tty, "-")) {
                struct termios t; int fd;
                close(in_fd);
                do { sleep(1); fd = open(tty, O_RDWR | O_NOCTTY); } while (fd < 0);
                if (!tcgetattr(fd, &t)) { cooked = t; cfmakeraw(&t); t.c_cc[VMIN] = 1; t.c_cc[VTIME] = 0; tcsetattr(fd, TCSANOW, &t); }
                in_fd = out_fd = fd;
            } else sleep(1);
            started = 0;
            strcpy(show_title, "WAITING"); show_text[0] = 0; show_pct = -1; draw(0);
            continue;
        }
        if (r == 0) { say("J49NANDIO 1"); continue; }
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
