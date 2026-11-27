/*
 * Dump a ramoops/pstore-RAM console zone from /dev/mem on the Nest (stock
 * kernel), to recover a modern kernel's boot log after a kexec + watchdog
 * warm-reset. ECC is disabled in the DT, so the console zone is plain text in a
 * simple persistent_ram ring buffer.
 *
 * Built static-musl via nixpkgs cross gcc (not zig). A Rust version of the same
 * logic is kept alongside as ramoops-read.rs for when a light Rust cross
 * toolchain is set up (nixpkgs rustc-cross needs a full bootstrap, so C is used
 * for now to match the proven kexec-tools/busybox build path).
 *
 * Layout (omap3-nest-j49.dts): console-only ramoops at 0x83f00000, 256 KB,
 * console zone at offset 0 (the confirmed-safe low window; stock clobbers the
 * top of RAM near 0x83fc0000). persistent_ram zone: u32 sig, u32 start,
 * u32 size, then data[capacity].
 *
 * Usage: ramoops-read [base_hex] [region_hex] [console_off_hex] [console_sz_hex]
 */
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <fcntl.h>
#include <unistd.h>
#include <sys/mman.h>

#define PERSISTENT_RAM_SIG 0x43474244u /* "DBGC" */

static uint32_t rd32(const uint8_t *p, size_t off) {
	return (uint32_t)p[off] | ((uint32_t)p[off+1] << 8) |
	       ((uint32_t)p[off+2] << 16) | ((uint32_t)p[off+3] << 24);
}

static unsigned long xhex(const char *s, unsigned long dflt) {
	if (!s) return dflt;
	if (!strncmp(s, "0x", 2)) s += 2;
	char *end; unsigned long v = strtoul(s, &end, 16);
	return (end == s) ? dflt : v;
}

static void dump_zone(const uint8_t *map, size_t region, size_t off,
                      size_t zone_sz, const char *label) {
	if (off + 12 > region) return;
	uint32_t sig = rd32(map, off);
	if (sig != PERSISTENT_RAM_SIG) {
		printf("[%s @ +0x%zx] empty (sig=0x%08x)\n", label, off, sig);
		return;
	}
	size_t cap = zone_sz - 12;
	size_t start = rd32(map, off + 4); if (start > cap) start = cap;
	size_t size  = rd32(map, off + 8); if (size  > cap) size  = cap;
	const uint8_t *data = map + off + 12;
	printf("===== %s @ +0x%zx (%zu bytes) =====\n", label, off, size);
	if (size < cap) {
		fwrite(data, 1, size, stdout);
	} else { /* wrapped: oldest bytes begin at start */
		fwrite(data + start, 1, cap - start, stdout);
		fwrite(data, 1, start, stdout);
	}
	printf("\n===== end %s =====\n", label);
}

int main(int argc, char **argv) {
	/* Defaults match omap3-nest-j49.dts: console-only ramoops at 0x83f00000,
	 * 256 KB, offset 0 (the confirmed-safe low window). */
	unsigned long base   = xhex(argc > 1 ? argv[1] : NULL, 0x83f00000UL);
	size_t region        = xhex(argc > 2 ? argv[2] : NULL, 0x00040000UL);
	size_t cons_off      = xhex(argc > 3 ? argv[3] : NULL, 0x00000000UL);
	size_t cons_sz       = xhex(argc > 4 ? argv[4] : NULL, 0x00040000UL);
	const size_t rec_sz  = 0x00020000UL;

	int fd = open("/dev/mem", O_RDONLY | O_SYNC);
	if (fd < 0) { perror("open /dev/mem"); return 1; }
	/* musl off_t is 64-bit, so base > 2 GB (0x83f00000) maps fine via mmap2 */
	void *p = mmap(NULL, region, PROT_READ, MAP_SHARED, fd, (off_t)base);
	if (p == MAP_FAILED) { perror("mmap /dev/mem"); close(fd); return 1; }
	const uint8_t *map = p;

	dump_zone(map, region, cons_off, cons_sz, "console");
	for (size_t off = 0, n = 0; off + rec_sz <= cons_off; off += rec_sz, n++) {
		if (rd32(map, off) == PERSISTENT_RAM_SIG) {
			char lbl[16]; snprintf(lbl, sizeof lbl, "dump%zu", n);
			dump_zone(map, region, off, rec_sz, lbl);
		}
	}
	munmap(p, region);
	close(fd);
	return 0;
}
