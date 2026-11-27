/*
 * stub.h -- shared definitions for the Nest J49 DFU flashstub.
 *
 * The flashstub is the one device-side piece of the installer that cannot
 * run Rust (tools/j49-screen): it executes before Linux exists, as the
 * second stage of the vendored x-load USB chain. Its NAND writer is a
 * verbatim port of the SPL env-write code in
 * meta-j49/recipes-bsp/u-boot/files/0003-omap3-spl-nest-j49-env-write.patch,
 * which wrote bit-exact stock BCH4 ECC on the real device on 2026-11-04.
 * The visuals are a C mirror of the j49-screen steel install screen
 * (tools/j49-screen/src/main.rs, render_install) -- same palette, same
 * ring, same words, see tools/dfu-install/ui/steel.h.
 *
 * Session-1 chain (see ../README.md):
 *   ROM <- x-load.bin (SRAM)  <- flashstub @0x80100000 + payload @0x82000000
 *   x-load jumps to 0x80100000; the stub writes the payload (a mkimage-
 *   wrapped U-Boot, -O u-boot -A arm -T firmware -a 0x80100000 -e 0,
 *   exactly like the stock spl0 image) into spl0+spl1 in STOCK BCH4 ECC, verifies, warm-resets.
 */
#ifndef STUB_H
#define STUB_H

typedef unsigned int u32;
typedef unsigned short u16;
typedef unsigned char u8;
typedef int s32;

#define REG32(a)        (*(volatile u32 *)(a))
#define REG16(a)        (*(volatile u16 *)(a))
#define REG8(a)         (*(volatile u8 *)(a))

/* ---- DRAM layout (all above the stub's own code at 0x80100000) ---- */
#define PAYLOAD_BASE    0x82000000UL  /* image + header, pushed by x-load */
#define VRAM_BASE       0x82800000UL  /* 320*320*4 = 400 KiB DISPC scanout */
#define STACK_TOP       0x80300000UL

/* ---- payload header, built on the host by ../mkpayload.py ---- */
#define NFD1_MAGIC      0x4E464431UL  /* "NFD1" */

struct nfd_range {
    u32 first_block;    /* absolute 128 KiB NAND block number */
    u32 n_blocks;       /* partition size in blocks */
};

struct nfd_hdr {
    u32 magic;          /* NFD1_MAGIC */
    u32 image_len;      /* payload image bytes, 0 < len <= n_blocks*0x20000 */
    u32 n_ranges;       /* 1..NFD_MAX_RANGES, written in this order */
    u32 reserved;
    struct nfd_range range[4];
    /* image bytes follow immediately at PAYLOAD_BASE + sizeof(*hdr) */
};

/* Hard allow-list: the ONLY partitions the stub will ever touch. ipl (0,1),
 * env (26..31), boot0 (32..39), root0 (48..71) and everything else are
 * refused by construction. Keep in sync with the flash map in
 * docs/flash-layout.md. */
#define NFD_MAX_RANGES  4
#define SPL0_BLK0       2
#define SPL0_NBLK       12
#define SPL1_BLK0       14
#define SPL1_NBLK       12
#define BLOCK_SIZE      0x20000UL   /* 128 KiB erase block */
#define PAGE_SIZE       2048
#define OOB_SIZE        64
#define PAGES_PER_BLK   (BLOCK_SIZE / PAGE_SIZE)   /* 64 */

/* ---- error codes (shown on the panel, mirror ui/steel.h) ---- */
enum {
    NFD_E_NONE = 0,
    NFD_E_HDR,          /* E1: header magic/ranges/length bad */
    NFD_E_BADBLOCK,     /* E2: bad block inside a target range */
    NFD_E_VERIFY,       /* E3: read-back mismatch after program */
    NFD_E_NAND,         /* E4: NAND does not become ready */
};

/* ---- stub.c / nand.c / panel.c API ---- */
void main(void);                    /* start.S calls this; never returns */
void stub_reset_warm(void);         /* PRM warm reset (patch 0003 verbatim) */
void stub_hang(int err);            /* ERROR screen + forever */

/* nand.c */
void nand_init(void);               /* GPMC CS0 x16 + NAND RESET */
int  nand_block_bad(u32 blk);       /* OOB word 0 of first page != 0xFFFF */
int  nand_erase_block(u32 blk);     /* 0 = ok, 1 = erase failed */
int  nand_prog_page(u32 page, const u8 *data, u8 ecc_out[28]); /* 0 = ok */
int  nand_verify_page(u32 page, const u8 *data, const u8 ecc_in[28]);

/* panel.c: best-effort visuals, never blocks the flash path */
void panel_init(void);              /* clocks, pads, TWL, LM3530, SPI, DSS */
void ui_phase(int phase, int pct);  /* 0..100 of the current phase */
void ui_phase_wait(void);           /* WAIT screen (waiting for payload) */
void ui_err(int err);               /* ERROR screen + specific code */
void wait_ms(int ms);               /* 32k-sync-timer based delay */
void beep(int n);                   /* n piezo blips (NEST_BEEP_DEBUG only) */
void wdt_arm(void);                 /* WDT2 ~12 s: hang -> warm reset */
void wdt_kick(void);                /* feed the watchdog */
/* phases -- same words as the j49-screen install screen */
enum {
    UI_WAIT = 0,   /* WAIT  */
    UI_ERASE,      /* ERASE */
    UI_WRITE,      /* WRITE */
    UI_VERIFY,     /* VERIFY */
    UI_DONE,       /* DONE */
    UI_ERR,        /* ERROR */
};

#endif /* STUB_H */
