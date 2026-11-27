/*
 * nand.c -- stock-BCH4 NAND writer for the J49 flashstub.
 *
 * Verbatim port of the silicon-proven sequence in
 * meta-j49/recipes-bsp/u-boot/files/0003-omap3-spl-nest-j49-env-write.patch
 * (env0 write, verified bit-exact on the device 2026-11-04: "ECC failed 0,
 * ECC corrected 0" from stock u-boot's own nanddump). Only the IO accessors
 * and the GPMC register addresses were made explicit; the BCH engine setup,
 * the 7-byte MSB-first packing, the OOB layout (bytes 0..35 = 0xFF free
 * area, 36..63 = 28 raw syndromes, NO erased-page polynomial fold) and the
 * page-program column/row cycle order are unchanged.
 *
 * Chip: Micron MT29F2G16ABBEAH4 (x16, 2048+64, 128 KiB blocks) on GPMC CS0.
 * GPMC timings: the generic omap3 NAND values that u-boot's gpmc_init()
 * uses (arch/arm/mach-omap2/mem-common.c, M_NAND_GPMC_CONFIG1..6) -- the
 * same values the recovery SPL ran with on this device.
 */
#include "stub.h"

/* ---- GPMC (base 0x6E000000, cs[0] at +0x60, struct gpmc in u-boot's
 * include/linux/mtd/omap_gpmc.h) ---- */
#define GPMC_CS0_CFG1      0x6E000060UL
#define GPMC_CS0_CFG7      0x6E000078UL
#define GPMC_NAND_CMD      0x6E00007CUL
#define GPMC_NAND_ADR      0x6E000080UL
#define GPMC_NAND_DAT      0x6E000084UL
#define GPMC_ECC_CONFIG    0x6E0001F4UL
#define GPMC_ECC_CONTROL   0x6E0001F8UL
#define GPMC_ECC_SIZE_CFG  0x6E0001FCUL
#define GPMC_BCH_RES0      0x6E000240UL   /* sector 0: RESULT0+0, RESULT1+4 */

/* u-boot M_NAND_GPMC_CONFIG1..6 for x16 NAND (mem-common.c) */
#define M_NAND_CFG1        0x00001800UL
#define M_NAND_CFG2        0x00141400UL
#define M_NAND_CFG3        0x00141400UL
#define M_NAND_CFG4        0x0F010F01UL
#define M_NAND_CFG5        0x010C1414UL
#define M_NAND_CFG6        0x1F0F0A80UL
/* CS0 window 0x30000000, 16 MiB (matches the kernel DTS ranges) */
#define M_NAND_CFG7        ((0xFUL << 8) | (0x30UL & 0x3F) | (1UL << 6))

/* GPMC BCH register bits -- identical to patch 0003 / u-boot omap_gpmc.c */
#define NEST_ECCSIZE1_SHIFT   22
#define NEST_ECCSIZE0_SHIFT   12
#define NEST_ECCCLEAR         (0x1UL << 8)
#define NEST_ECCRESULTREG1    (0x1UL << 0)

static u8 nand_wait_ready(void)
{
    int i;

    REG8(GPMC_NAND_CMD) = 0x70;         /* READ STATUS */
    for (i = 0; i < 4000000; i++) {
        u8 st = REG8(GPMC_NAND_DAT);
        if (st & 0x40)                   /* bit6 = ready */
            return st;                   /* bit0 = program/erase FAILED */
    }
    return 0x01;                         /* never ready: report failed */
}

/*
 * Program the GPMC BCH engine to compute ONE BCH4 syndrome over the next
 * 512-byte sector streamed through the data port: wrap mode 6 (main area
 * only), bch_type 0 (BCH4), dev_width 1 (x16). Bit-identical to patch 0003.
 */
static void bch4_start(void)
{
    u32 val;

    REG32(GPMC_ECC_CONTROL) = NEST_ECCRESULTREG1;
    val = (0x20UL << NEST_ECCSIZE1_SHIFT) | (0x0UL << NEST_ECCSIZE0_SHIFT);
    REG32(GPMC_ECC_SIZE_CFG) = val;
    val = (1UL << 16)      /* enable BCH */
        | (0UL << 12)      /* bch_type 0 = BCH4 */
        | (6UL <<  8)      /* wrap mode 6: main area only */
        | (1UL <<  7)      /* dev_width 1 = x16 */
        | (0UL <<  4)      /* 1 sector */
        | (0UL <<  1)      /* ECC on CS0 */
        | (1UL);           /* enable ECC */
    REG32(GPMC_ECC_CONFIG) = val;
    REG32(GPMC_ECC_CONTROL) = NEST_ECCCLEAR | NEST_ECCRESULTREG1;
}

/*
 * Read the 52-bit BCH4 remainder and pack it MSB-first into 7 bytes: the
 * STOCK raw layout, no polynomial fold. Bit-identical to patch 0003 (which
 * itself mirrors u-boot/kernel _omap_calculate_ecc BCH4 packing).
 */
static void bch4_read(u8 *ecc)
{
    u32 hi = REG32(GPMC_BCH_RES0 + 4);   /* BCH_RESULT_1 */
    u32 lo = REG32(GPMC_BCH_RES0);       /* BCH_RESULT_0 */

    ecc[0] = (hi >> 12) & 0xff;
    ecc[1] = (hi >>  4) & 0xff;
    ecc[2] = ((hi & 0xf) << 4) | ((lo >> 28) & 0xf);
    ecc[3] = (lo >> 20) & 0xff;
    ecc[4] = (lo >> 12) & 0xff;
    ecc[5] = (lo >>  4) & 0xff;
    ecc[6] = (lo & 0xf)  << 4;
}

void nand_init(void)
{
    /* GPMC globals exactly as gpmc_init() (mem-common.c) does them -- the
     * missing piece in the first port was GPMC_CONFIG 0x12: WITHOUT it CS0
     * stays write-protected and erase/program silently fail (the stub then
     * ended in E2/E3 before spl0 and stock booted back). */
    REG32(0x6E000010UL) = 0x8;          /* sysconfig */
    REG32(0x6E000018UL) = 0;            /* irqstatus */
    REG32(0x6E00001CUL) = 0;            /* irqenable */
    REG32(0x6E000040UL) = 0x1FF0;       /* timeout_control */
    REG32(0x6E000050UL) = 0x12;         /* config: CS0 write protect off, NAND */

    /* GPMC CS0 for x16 NAND, exactly as set_gpmc_cs0() does: disable the
     * CS while reconfiguring, then enable the 0x30000000/16M window. DRAM
     * itself is up -- x-load init'd it before receiving us. */
    REG32(GPMC_CS0_CFG7) = 0;
    REG32(GPMC_CS0_CFG1 + 0x00) = M_NAND_CFG1;
    REG32(GPMC_CS0_CFG1 + 0x04) = M_NAND_CFG2;
    REG32(GPMC_CS0_CFG1 + 0x08) = M_NAND_CFG3;
    REG32(GPMC_CS0_CFG1 + 0x0C) = M_NAND_CFG4;
    REG32(GPMC_CS0_CFG1 + 0x10) = M_NAND_CFG5;
    REG32(GPMC_CS0_CFG1 + 0x14) = M_NAND_CFG6;
    REG32(GPMC_CS0_CFG7) = M_NAND_CFG7;

    /* RESET (0xFF): the ROM's boot reads may leave the chip mid-operation
     * (the NAND RESET was essential on the recovery SPL, patch 0002). */
    REG8(GPMC_NAND_CMD) = 0xFF;
    nand_wait_ready();
}

int nand_block_bad(u32 blk)
{
    u32 page = blk * PAGES_PER_BLK;
    u16 marker;
    u8 st;

    /* READ the OOB word 0 (bad-block marker, x16 large page): column 1024
     * words = col_lo 0x00, col_hi 0x04, then row, then 0x30 read confirm. */
    REG8(GPMC_NAND_CMD) = 0x00;
    REG8(GPMC_NAND_ADR) = 0x00;
    REG8(GPMC_NAND_ADR) = 0x04;
    REG8(GPMC_NAND_ADR) = page & 0xff;
    REG8(GPMC_NAND_ADR) = (page >>  8) & 0xff;
    REG8(GPMC_NAND_ADR) = (page >> 16) & 0xff;
    REG8(GPMC_NAND_CMD) = 0x30;
    st = nand_wait_ready();
    if (st & 0x01)
        return 1;                        /* command failed: treat as bad */
    marker = REG16(GPMC_NAND_DAT);
    return marker != 0xFFFF;
}

int nand_erase_block(u32 blk)
{
    u32 page = blk * PAGES_PER_BLK;

    REG8(GPMC_NAND_CMD) = 0x60;              /* ERASE setup */
    REG8(GPMC_NAND_ADR) = page & 0xff;
    REG8(GPMC_NAND_ADR) = (page >>  8) & 0xff;
    REG8(GPMC_NAND_ADR) = (page >> 16) & 0xff;
    REG8(GPMC_NAND_CMD) = 0xD0;              /* ERASE confirm */
    return nand_wait_ready() & 0x01;         /* 0 = ok */
}

int nand_prog_page(u32 page, const u8 *data, u8 ecc_out[28])
{
    u8 ecc[28];
    u8 st;
    int s, k;

    REG8(GPMC_NAND_CMD) = 0x80;              /* PROGRAM PAGE */    REG8(GPMC_NAND_ADR) = 0x00;              /* col[7:0] = 0 */
    REG8(GPMC_NAND_ADR) = 0x00;              /* col[10:8] = 0 */
    REG8(GPMC_NAND_ADR) = page & 0xff;
    REG8(GPMC_NAND_ADR) = (page >>  8) & 0xff;
    REG8(GPMC_NAND_ADR) = (page >> 16) & 0xff;

    for (s = 0; s < 4; s++) {
        const u8 *p = data + s * 512;

        bch4_start();
        for (k = 0; k < 512; k += 2)        /* 256 x16 words */
            REG16(GPMC_NAND_DAT) = (u16)(p[k] | (p[k + 1] << 8));
        bch4_read(&ecc[s * 7]);
    }

    for (k = 0; k < 64; k += 2) {            /* 32 x16 OOB words */
        u8 b0 = (k     >= 36) ? ecc[k     - 36] : 0xFF;
        u8 b1 = (k + 1 >= 36) ? ecc[k + 1 - 36] : 0xFF;

        REG16(GPMC_NAND_DAT) = (u16)(b0 | (b1 << 8));
    }

    REG8(GPMC_NAND_CMD) = 0x10;              /* PROGRAM confirm */
    st = nand_wait_ready();                  /* bit0 = program failed */

    if (ecc_out && !(st & 0x01))
        for (s = 0; s < 28; s++)
            ecc_out[s] = ecc[s];
    return st & 0x01;
}

int nand_verify_page(u32 page, const u8 *data, const u8 ecc_in[28])
{
    /* raw sequential read of main+OOB from column 0; raw reads carry no ECC
     * interpretation, so this sees back exactly the bytes we programmed,
     * including the 28 syndromes we placed at OOB 36..63. */
    static u8 buf[PAGE_SIZE + OOB_SIZE];
    u32 i;

    REG8(GPMC_NAND_CMD) = 0x00;
    REG8(GPMC_NAND_ADR) = 0x00;
    REG8(GPMC_NAND_ADR) = 0x00;
    REG8(GPMC_NAND_ADR) = page & 0xff;
    REG8(GPMC_NAND_ADR) = (page >>  8) & 0xff;
    REG8(GPMC_NAND_ADR) = (page >> 16) & 0xff;
    REG8(GPMC_NAND_CMD) = 0x30;
    nand_wait_ready();

    for (i = 0; i < PAGE_SIZE + OOB_SIZE; i += 2) {
        u16 w = REG16(GPMC_NAND_DAT);
        buf[i]     = w & 0xff;
        buf[i + 1] = w >> 8;
    }

    for (i = 0; i < PAGE_SIZE; i++)
        if (buf[i] != data[i])
            return 1;
    for (i = 0; i < 36; i++)
        if (buf[PAGE_SIZE + i] != 0xFF)
            return 2;
    for (i = 0; i < 28; i++)
        if (buf[PAGE_SIZE + 36 + i] != ecc_in[i])
            return 3;
    return 0;
}

void stub_reset_warm(void)
{
    /* PRM warm reset, verbatim from patch 0003 (RST_GS at 0x48307250) */
    REG32(0x48307250UL) = REG32(0x48307250UL) | 0x2;
    for (;;)
        ;
}
