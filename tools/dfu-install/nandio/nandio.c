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
 *   (start)                        nandio sends "J49NANDIO 1" once a second until a command comes
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
    }
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
        if (!is_bad(fd, off) && erase_block(fd, p, off)) failed++;
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

int main(int argc, char **argv)
{
    char line[160];
    const char *tty = argc > 1 ? argv[1] : "/dev/ttyGS0";
    int started = 0;
    fake = getenv("NANDIO_FAKE");
    crc_init();
    if (!fake) {
        struct termios t;
        int fd = open(tty, O_RDWR | O_NOCTTY);
        if (fd < 0) { perror(tty); return 1; }
        /* raw: the line carries binary data in both directions */
        if (!tcgetattr(fd, &t)) { cfmakeraw(&t); t.c_cc[VMIN] = 1; t.c_cc[VTIME] = 0; tcsetattr(fd, TCSANOW, &t); }
        in_fd = out_fd = fd;
    }
    if (load_parts()) { say("ERR no partition table"); return 1; }
    note("ready: waiting for the installer on USB");
    for (;;) {
        char cmd[16], a[64], c[16]; unsigned long n = 0; int k;
        int r = get_line(line, sizeof line, started ? 0 : 1000);
        if (r < 0) return 0;
        if (r == 0) { say("J49NANDIO 1"); continue; }
        started = 1;
        cmd[0] = a[0] = c[0] = 0;
        k = sscanf(line, "%15s %63s %lu %15s", cmd, a, &n, c);
        if (k < 1) continue;
        if (!strcmp(cmd, "INFO")) cmd_info();
        else if (!strcmp(cmd, "READ") && k >= 2) cmd_read(a);
        else if (!strcmp(cmd, "ERASE") && k >= 2) cmd_erase(a);
        else if (!strcmp(cmd, "WRITE") && k == 4) cmd_write(a, n, c);
        else if (!strcmp(cmd, "REBOOT")) {
            say("OK");
            note("restart");
            if (fake) return 0;
            sync(); sleep(1);
            execl("/sbin/reboot", "reboot", "-f", (char *)NULL);
            execl("/bin/busybox", "busybox", "reboot", "-f", (char *)NULL);
            return 1;
        } else say("ERR unknown command");
    }
}
