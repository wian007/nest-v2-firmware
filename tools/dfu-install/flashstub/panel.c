/*
 * panel.c -- round LCD bring-up + the steel installer screen, for the part
 * of the installer that runs before Linux exists (the flashstub).
 *
 * The renderer is a C mirror of the j49-screen install screen
 * (tools/j49-screen/src/main.rs :: render_install): same palette, same
 * 320x320 XRGB8888 frame, same 5x7 font, same 60-tick progress ring, same
 * words (WAIT/ERASE/WRITE/VERIFY/DONE/ERROR). Integer-only: the
 * stub never enables the VFP coprocessor, so no floats anywhere.
 *
 * Bring-up is best-effort: if anything here fails, panel_ok stays 0 and the
 * flash path runs blind. The USB side is the host's signal then (ROM
 * disappears = stub is running; the DFU gadget ~10 s after warm reset =
 * success; the stock mass-storage gadget = spl0 write failed).
 *
 * Register facts and their in-tree sources:
 *  - PRCM enables: u-boot-2024.07 arch/arm/mach-omap2/omap3/clock.c
 *    (I2C1-3 = FCLKEN1_CORE bits 15..17, McSPI1 = bit 18, GPIO4/6 =
 *    FCLKEN_PER 0x8000/0x20000, DSS = FCLKEN_DSS 0x5 + ICLKEN_DSS 0x1,
 *    32k sync timer = ICLKEN_WKUP bit 2)
 *  - padconf values: our own kernel DTS (meta-j49/.../omap3-nest-j49.dts
 *    hog + dss_dpi_pins + mcspi1_pins), from the stock pad dump
 *  - TWL4030 LDOs: u-boot include/twl4030.h (VAUX1 0x75/0x72, VAUX2
 *    0x79/0x76, VPLL2 0x91/0x8E) + kernel twl-regulator.c VSEL tables
 *    (VAUX1 3.0V = 0x04, VAUX2 2.8V = 0x09, VPLL2 1.8V = 0x05), I2C addr
 *    0x4B on I2C1
 *  - LM3530: kernel drivers/leds/leds-lm3530.c (GEN_CONFIG 0x10 = 0x1d
 *    per our DT comment, BRT_CTRL 0xA0), I2C addr 0x36 on I2C3, HWEN =
 *    gpio177, power = VAUX1
 *  - panel init command table: our kernel patch
 *    0004-drm-panel-samsung-s6d05a1-nest-j49.patch (vendor 2.6.37 sequence)
 *  - McSPI/I2C/DISPC programming: u-boot drivers/spi/omap3_spi.c,
 *    drivers/i2c/omap24xx_i2c.c, drivers/video/omap3_dss.c + dss.h
 */
#include "stub.h"
#include "font5x7.h"

/* ------------------------------------------------------------------ util */

static int panel_ok;        /* 0 until the full chain up */

void wait_ms(int ms)
{
    /* 32KSYNC counter @ 0x48320010 (u-boot s32k_base->s32k_cr), 32768 Hz.
     * BOUNDED: if the counter does not tick (wrong gate/address), give up
     * after a loop budget instead of hanging the whole installer. */
    u32 start = REG32(0x48320010UL);
    u32 ticks = (u32)ms * 33;
    u32 budget = ticks * 32 + 1000;

    while ((REG32(0x48320010UL) - start) < ticks && --budget)
        ;
}

/* gcc still emits memcpy/memset under -ffreestanding for struct/init */
void *memset(void *d, int c, unsigned long n)
{
    u8 *p = d;
    while (n--)
        *p++ = (u8)c;
    return d;
}
void *memcpy(void *d, const void *s, unsigned long n)
{
    u8 *dp = d;
    const u8 *sp = s;
    while (n--)
        *dp++ = *sp++;
    return d;
}

/* --------------------------------------------------------------- clocks */

static void clocks_on(void)
{
    /* CORE: I2C1/2/3 (bits 15..17) + McSPI1 (bit 18) */
    REG32(0x48004A00UL) |= 0x00078000;    /* CM_FCLKEN1_CORE */
    REG32(0x48004A10UL) |= 0x00078000;    /* CM_ICLKEN1_CORE */
    /* PER: GPIO4 (gpio109) + GPIO6 (gpio177) + GPT8 (piezo debug beeps:
     * timer8 = bit9 0x200; clksel_per bit6 0x40 selects the 32 kHz source,
     * matching the kernel's piezo_pwm ti,clock-source = <0x01>) */
    REG32(0x48005000UL) |= (0x8000 | 0x20000 | 0x200);  /* CM_FCLKEN_PER */
    REG32(0x48005010UL) |= (0x8000 | 0x20000 | 0x200);  /* CM_ICLKEN_PER */
    REG32(0x48005040UL) |= 0x40;                        /* CM_CLKSEL_PER: GPT8 32k */
    /* WKUP: 32k sync timer (bit 2) for wait_ms + WDT2 (bit 5) */
    REG32(0x48004C00UL) |= 0x20;          /* CM_FCLKEN_WKUP: WDT2 */
    REG32(0x48004C10UL) |= 0x4 | 0x20;    /* CM_ICLKEN_WKUP: 32k + WDT2 */
    wait_ms(1);
    /* DSS: functional (DSS1/DSS2) + interface clocks, AFTER the others so
     * an accidental gate of the I2C used for TWL config can never happen. */
    REG32(0x48004E00UL) |= 0x5;           /* CM_FCLKEN_DSS */
    REG32(0x48004E10UL) |= 0x1;           /* CM_ICLKEN_DSS */
    wait_ms(1);
}

/* ----------------------------------------------------------------- pads */

static const struct { u16 off; u16 val; } pads[] = {
    /* DSS DPI (dss_pclk..dss_data23, mode 0 output) -- DTS dss_dpi_pins */
    { 0x00D4, 0x0000 }, { 0x00D6, 0x0000 }, { 0x00D8, 0x0000 },
    { 0x00DA, 0x0000 }, { 0x00DC, 0x0000 }, { 0x00DE, 0x0000 },
    { 0x00E0, 0x0000 }, { 0x00E2, 0x0000 }, { 0x00E4, 0x0000 },
    { 0x00E6, 0x0000 }, { 0x00E8, 0x0000 }, { 0x00EA, 0x0000 },
    { 0x00EC, 0x0000 }, { 0x00EE, 0x0000 }, { 0x00F0, 0x0000 },
    { 0x00F2, 0x0000 }, { 0x00F4, 0x0000 }, { 0x00F6, 0x0000 },
    { 0x00F8, 0x0000 }, { 0x00FA, 0x0000 }, { 0x00FC, 0x0000 },
    { 0x00FE, 0x0000 }, { 0x0100, 0x0000 }, { 0x0102, 0x0000 },
    { 0x0104, 0x0000 }, { 0x0106, 0x0000 }, { 0x0108, 0x0000 },
    { 0x010A, 0x0000 },
    /* McSPI1: clk in, simo out, somi in, cs0 in+pullup -- DTS mcspi1_pins */
    { 0x01C8, 0x0100 }, { 0x01CA, 0x0000 }, { 0x01CC, 0x0100 },
    { 0x01CE, 0x0108 },
    /* I2C1 (TWL4030) + I2C3 (LM3530), input mode 0 -- DTS nest_core_pins */
    { 0x018A, 0x0100 }, { 0x018C, 0x0100 },   /* i2c1_scl/sda */
    { 0x0192, 0x0100 }, { 0x0194, 0x0100 },   /* i2c3_scl/sda */
    /* gpio109 LCD #RESET (output, DTS cam_d10), gpio177 LM3530 HWEN */
    { 0x00FA, 0x1604 }, { 0x01A4, 0x0204 },
    /* piezo: gpmc_ncs7 -> gpt8_pwm_evt (mode3), DTS hog 0x008c */
    { 0x008C, 0x0003 },
};

static void pads_on(void)
{
    unsigned i;

    for (i = 0; i < sizeof(pads) / sizeof(pads[0]); i++)
        REG16(0x48002000UL + pads[i].off) = pads[i].val;
}

/* ------------------------------------------------------------------ i2c */

/* OMAP3 I2C (IP v1) register offsets, from u-boot reg_map_ip_v1 */
#define I2C_STAT  0x08
#define I2C_CNT   0x18
#define I2C_DATA  0x1C
#define I2C_SYSC  0x20
#define I2C_SYSS  0x10
#define I2C_CON   0x24
#define I2C_SA    0x2C
#define I2C_PSC   0x30
#define I2C_SCLL  0x34
#define I2C_SCLH  0x38

#define CON_EN    (1u << 15)
#define CON_MST   (1u << 10)
#define CON_TRX   (1u << 9)
#define CON_STP   (1u << 1)
#define CON_STT   (1u << 0)
#define ST_BB     (1u << 12)
#define ST_XRDY   (1u << 4)
#define ST_ARDY   (1u << 2)
#define ST_NACK   (1u << 1)

static int i2c_init_bus(unsigned long base)
{
    u32 timeout = 100000;

    if (REG32(base + I2C_CON) & CON_EN)
        REG32(base + I2C_CON) = 0;
    REG32(base + I2C_SYSC) = 0x2;         /* soft reset */
    wait_ms(1);
    REG32(base + I2C_CON) = CON_EN;
    while (!(REG32(base + I2C_SYSS) & 1) && --timeout)
        ;
    if (!timeout)
        return 1;

    /* 100 kHz from the 96 MHz functional clock: psc = 7 (12 MHz internal),
     * scll = 60, sclh = 60 with the u-boot trims (-6/-6 + base 12? -> the
     * classic omap3 values). u-boot's omap24_i2c_setspeed at 100k with
     * I2C_IP_CLK 96 MHz lands on psc=7, scll=0x33+? -- take the widely
     * used pair for 96M/100k: scll 0x2F+12? Use the values the stock
     * bootloader family uses: psc 7, scll 0x33, sclh 0x33. */
    REG32(base + I2C_PSC)  = 7;
    REG32(base + I2C_SCLL) = 0x33;
    REG32(base + I2C_SCLH) = 0x33;
    REG32(base + 0x28)     = 0;           /* OA: our own address, unused */
    REG32(base + 0x04)     = 0x1E;        /* IE: XRDY|RRDY|ARDY|NACK|AL */
    REG32(base + I2C_STAT) = 0xFFFF;      /* clear all events */

    /* wait for bus free */
    timeout = 100000;
    while ((REG32(base + I2C_STAT) & ST_BB) && --timeout)
        ;
    return timeout ? 0 : 1;
}

/* one write transaction: [reg, val] to 7-bit addr on `base` */
static int i2c_write2(unsigned long base, u8 addr, u8 reg, u8 val)
{
    u32 timeout = 100000;

    if (REG32(base + I2C_STAT) & ST_BB)
        return 1;

    REG32(base + I2C_CNT) = 2;
    REG32(base + I2C_SA)  = addr;
    REG32(base + I2C_CON) = CON_EN | CON_MST | CON_TRX | CON_STT | CON_STP;

    while (!((REG32(base + I2C_STAT) & (ST_XRDY | ST_NACK))) && --timeout)
        ;
    if (!timeout || (REG32(base + I2C_STAT) & ST_NACK)) {
        REG32(base + I2C_CON) = CON_EN | CON_MST | CON_STP;
        REG32(base + I2C_STAT) = 0xFFFF;
        return 1;
    }
    REG32(base + I2C_DATA) = reg;

    timeout = 100000;
    while (!((REG32(base + I2C_STAT) & (ST_XRDY | ST_NACK))) && --timeout)
        ;
    if (!timeout || (REG32(base + I2C_STAT) & ST_NACK)) {
        REG32(base + I2C_CON) = CON_EN | CON_MST | CON_STP;
        REG32(base + I2C_STAT) = 0xFFFF;
        return 1;
    }
    REG32(base + I2C_DATA) = val;

    timeout = 100000;
    while (!((REG32(base + I2C_STAT) & (ST_ARDY | ST_NACK))) && --timeout)
        ;
    REG32(base + I2C_CON) = CON_EN | CON_MST | CON_STP;
    REG32(base + I2C_STAT) = 0xFFFF;
    return (!timeout || (REG32(base + I2C_STAT) & ST_NACK)) ? 1 : 0;
}

/* TWL4030 LDO enable on I2C1 (0x48070000): vsel into DEDICATED, then group
 * P1|P2|P3 into DEV_GRP, exactly like u-boot twl4030_pmrecv_vsel_cfg(). */
static int twl_ldo(u8 dedicated, u8 vsel, u8 dev_grp)
{
    if (i2c_write2(0x48070000UL, 0x4B, dedicated, vsel))
        return 1;
    return i2c_write2(0x48070000UL, 0x4B, dev_grp, 0xE0);
}

/* ----------------------------------------------------------------- gpio */

#define GPIO4_BASE 0x49054000UL   /* gpio96..127: gpio109 = LCD #RESET */
#define GPIO6_BASE 0x49058000UL   /* gpio160..191: gpio177 = LM3530 HWEN */
#define GPIO_OE        0x34
#define GPIO_CLEARDATAOUT 0x90
#define GPIO_SETDATAOUT  0x94

static void gpio_out(unsigned long bank, int bit, int high)
{
    REG32(bank + GPIO_OE) &= ~(1u << bit);
    if (high)
        REG32(bank + GPIO_SETDATAOUT) = (1u << bit);
    else
        REG32(bank + GPIO_CLEARDATAOUT) = (1u << bit);
}

/* ------------------------------------------------- hardware watchdog */
/* WDT2 @ 0x48314000, armed early with ~12 s and kicked at milestones and
 * during the flash loops. A hang therefore becomes a WARM RESET -> the ROM
 * falls back to USB peripheral boot (fresh, responsive), instead of a stale
 * gadget that needs a physical power-off. Register sequence copied from
 * the kernel omap_wdt.c (disable/init/load/enable/kick), all polls bounded.
 * A warm reset clears the WDT again, so stock is unaffected. */
#define WDT2_BASE   0x48314000UL
#define WDT_SYSCFG  0x10
#define WDT_CNTRL   0x24
#define WDT_LDR     0x2C
#define WDT_TGR     0x30
#define WDT_WPS     0x34
#define WDT_SPR     0x48

static u32 wdt_pat;

static void wdt_wait(u32 bit)
{
    u32 budget = 100000;
    while ((REG32(WDT2_BASE + WDT_WPS) & bit) && --budget)
        ;
}

void wdt_kick(void)
{
    wdt_wait(0x08);
    wdt_pat = ~wdt_pat;
    REG32(WDT2_BASE + WDT_TGR) = wdt_pat;
    wdt_wait(0x08);
}

void wdt_arm(void)
{
    REG32(WDT2_BASE + WDT_SYSCFG) = 0;     /* no idle, always clocked */
    REG32(WDT2_BASE + WDT_SPR) = 0xAAAA;   /* disable */
    wdt_wait(0x10);
    REG32(WDT2_BASE + WDT_SPR) = 0x5555;
    wdt_wait(0x10);
    wdt_wait(0x01);
    REG32(WDT2_BASE + WDT_CNTRL) = (1u << 5);      /* PTV = 0 */
    wdt_wait(0x01);
    wdt_wait(0x04);
    REG32(WDT2_BASE + WDT_LDR) = 0xFFFFFFFFu - 12u * 32768u + 1u;
    wdt_wait(0x04);
    wdt_kick();
    REG32(WDT2_BASE + WDT_SPR) = 0xBBBB;   /* enable */
    wdt_wait(0x10);
    REG32(WDT2_BASE + WDT_SPR) = 0x4444;
    wdt_wait(0x10);
}

/* --------------------------------------------------------- piezo beeps */
/* Audible bring-up diagnostics: GPT8 in PWM toggle mode on the piezo pad
 * (32 kHz clock, 2 kHz tone like stock's click). beep(n) = n short blips.
 * Milestone table (last heard count = how far the stub got):
 *   1 clocks+pads up   2 TWL LDOs ok    3 LM3530 ok
 *   4 McSPI ok         5 DISPC/panel up 6 header validated
 *   7 NAND init ok     8 bad-block scan clean
 *   9 all erase/write/verify done       10 error (any E-code)
 */
#define GPT8_BASE   0x4903E000UL
#define GPT_TCLR    0x24
#define GPT_TCRR    0x28
#define GPT_TLDR    0x2C
#define GPT_TMAR    0x38

static int clocks_up;

void beep(int n)
{
#ifdef NEST_BEEP_DEBUG
    int i, b;

    if (!clocks_up)
        return;
    for (i = 0; i < n; i++) {
        REG32(GPT8_BASE + GPT_TCLR) = 0;
        REG32(GPT8_BASE + GPT_TLDR) = 16;   /* 32 kHz / 16 = 2 kHz */
        REG32(GPT8_BASE + GPT_TMAR) = 8;    /* 50% duty */
        REG32(GPT8_BASE + GPT_TCRR) = 0;
        /* ST | AR | PT (toggle) | trigger overflow&compare (0x2<<10) */
        REG32(GPT8_BASE + GPT_TCLR) = 0x1 | 0x2 | 0x1000 | (0x2 << 10);
        wait_ms(60);
        REG32(GPT8_BASE + GPT_TCLR) = 0;
        for (b = 0; b < 120; b++)
            wait_ms(1);
    }
#else
    (void)n;
#endif
}

/* ----------------------------------------------------------------- spi */

/* McSPI1 @ 0x48098000, channel 0 (the panel CS) */
#define MCSPI1     0x48098000UL
#define MCSPI_SYSCONFIG 0x10
#define MCSPI_SYSSTATUS 0x14
#define MCSPI_MODULCTRL 0x28
#define MCSPI_CH0CONF   0x2C
#define MCSPI_CH0STAT   0x30
#define MCSPI_CH0CTRL   0x34
#define MCSPI_TX0        0x38

/* 9-bit words, mode 3, <= 10 MHz: 48 MHz fck / CLKD 8 = 6 MHz */
#define CHCONF_BASE  ((8u << 7) | 0x3u | (2u << 2) | (1u << 16))
#define CHCONF_TX   (CHCONF_BASE | (1u << 13) | (1u << 20))
#define ST_TXS      (1u << 1)
#define ST_EOT      (1u << 2)

static int spi_init(void)
{
    u32 timeout = 100000;

    REG32(MCSPI1 + MCSPI_SYSCONFIG) = 0x2;   /* SOFTRESET */
    while (!(REG32(MCSPI1 + MCSPI_SYSSTATUS) & 1) && --timeout)
        ;
    if (!timeout)
        return 1;
    REG32(MCSPI1 + MCSPI_SYSCONFIG) = 0x11;  /* AUTOIDLE | SMARTIDLE */
    REG32(MCSPI1 + MCSPI_MODULCTRL)  = 0x1;  /* SINGLE-channel master */

    /* WL 9, PHA|POL (mode 3), CLKD 8, DPE0 (D1 out), IS (input select) */
    REG32(MCSPI1 + MCSPI_CH0CONF) = CHCONF_BASE;
    return 0;
}

static void spi_words(const u16 *w, int n)
{
    int i;

    REG32(MCSPI1 + MCSPI_CH0CONF) = CHCONF_TX;
    REG32(MCSPI1 + MCSPI_CH0CTRL) = 1;        /* channel enable */
    for (i = 0; i < n; i++) {
        u32 timeout = 100000;
        while (!(REG32(MCSPI1 + MCSPI_CH0STAT) & ST_TXS) && --timeout)
            ;
        REG32(MCSPI1 + MCSPI_TX0) = w[i];
    }
    /* transfer end: EOT with TXS, then park the channel */
    {
        u32 timeout = 100000;
        while (!(REG32(MCSPI1 + MCSPI_CH0STAT) & (ST_EOT | ST_TXS)) && --timeout)
            ;
    }
    REG32(MCSPI1 + MCSPI_CH0CTRL) = 0;
    REG32(MCSPI1 + MCSPI_CH0CONF) = CHCONF_BASE;
}

/* ---------------------------------------------------------------- panel */

/* the vendor init sequence, verbatim from kernel patch 0004
 * (panel-samsung-s6d05a1.c s6d05a1_init) -- cmd byte + params, each word
 * 9 bits with D/CX in bit 8 */
static const struct { u8 cmd; u8 np; const u8 p[19]; } panel_seq[] = {
    { 0x36, 1, { 0xD0 } },                                        /* MADCTL */
    { 0xF0, 2, { 0x5A, 0x5A } },                                  /* PASSWD1 */
    { 0xF1, 2, { 0x5A, 0x5A } },                                  /* PASSWD2 */
    { 0xF2, 19, { 0x27, 0x30, 0x03, 0x0A, 0x08, 0x08, 0x08, 0x00,  /* DISCTL */
                  0x08, 0x08, 0x00, 0x01, 0x00, 0x00, 0x54, 0x0A,
                  0x08, 0x0A, 0x08 } },
    { 0xF4, 14, { 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  /* PWRCTL */
                  0x00, 0x66, 0x02, 0x00, 0x52, 0x02 } },
    { 0xF5, 12, { 0x00, 0x5C, 0x5D, 0x00, 0x00, 0x04, 0x00, 0x00,  /* VCMCTL */
                  0x04, 0x00, 0x2A, 0x21 } },
    { 0xF6, 9, { 0x01, 0x00, 0x08, 0x03, 0x01, 0x01, 0x01, 0x00,  /* SRCCTL */
                 0x00 } },
    { 0xF7, 5, { 0x48, 0x81, 0x20, 0x12, 0x01 } },                /* IFCTL */
    { 0xF8, 2, { 0x11, 0x00 } },                                  /* PANELCTL */
    { 0xF9, 1, { 0x17 } },                                        /* GAMMASEL */
    { 0xFA, 16, { 0x00, 0x02, 0x00, 0x21, 0x2A, 0x2D, 0x2E, 0x22, /* PGAMMA */
                  0x28, 0x2F, 0x3C, 0x3F, 0x34, 0x00, 0x00, 0x00 } },
    { 0xFB, 16, { 0x00, 0x02, 0x34, 0x3F, 0x3C, 0x2F, 0x28, 0x22, /* NGAMMA */
                  0x2E, 0x2D, 0x2A, 0x21, 0x00, 0x00, 0x00, 0x00 } },
    { 0x3A, 1, { 0x77 } },                                        /* COLMOD 24bpp */
    { 0x2A, 4, { 0x00, 0x00, 0x01, 0x3F } },                      /* CASET */
    { 0x2B, 4, { 0x00, 0x00, 0x01, 0x3F } },                      /* PASET */
};
static const struct { u8 cmd; } panel_seq2[] = {
    { 0x11 },   /* SLPOUT */
    { 0x38 },   /* IDMOFF */
    { 0x29 },   /* DISPON */
};

static void panel_reset(void)
{
    gpio_out(GPIO4_BASE, 13, 1);    /* deassert, unknown initial state */
    wait_ms(2);
    gpio_out(GPIO4_BASE, 13, 0);    /* assert #RESET >= 5 ms */
    wait_ms(10);
    gpio_out(GPIO4_BASE, 13, 1);    /* deassert, panel boots */
    wait_ms(120);
}

/* ----------------------------------------------------------- DSS/DISPC */

/* 320x320, 7.18 MHz mode (patch 0004): hsw2 hfp16 hbp16, vsw2 vfp8 vbp8,
 * DE active low (IEO), pixels on falling edge (IPC), syncs on the other
 * edge (ONOFF = 0). 24-bit DPI. 24bpp unpacked gfx = the j49-screen frame
 * layout, byte-for-byte.  Divisors assume the canonical 96 MHz dss1_alwon
 * fck the stock boot chain sets (x-load's DPLL4 config) -> 7.38 MHz pixel
 * clock, ~2.8% over the vendor mode; the panel booster takes whatever
 * clock it gets (BOOST_CLK_SEL_EXT).  If the panel stays dark on hardware,
 * DISPC_DIVISOR is the first knob to check. */
#define DISPC      0x48050400UL
#define DSS_BASE   0x48050000UL
#define VRAM       ((u32 *)VRAM_BASE)
#define FW 320
#define FH 320

static int dispc_init(void)
{
    u32 budget;

    REG32(DSS_BASE + 0x10) = 0x2;             /* DSS softreset */
    budget = 100000;
    while (!(REG32(DSS_BASE + 0x14) & 1) && --budget)
        ;
    if (!budget)
        return 1;                             /* DSS did not come out of reset */

    REG32(DISPC + 0x10) = 0x2;                /* DISPC softreset */
    budget = 100000;
    while (!(REG32(DISPC + 0x14) & 1) && --budget)
        ;
    if (!budget)
        return 1;

    REG32(DISPC + 0x64) = (15UL << 20) | (15UL << 8) | 1;   /* timing_h */
    REG32(DISPC + 0x68) = (8UL  << 20) | (8UL  << 8) | 1;   /* timing_v */
    REG32(DISPC + 0x6C) = (1UL << 15) | (1UL << 14);        /* pol_freq: IEO|IPC */
    REG32(DISPC + 0x70) = (1UL << 16) | 13;                 /* divisor */
    REG32(DISPC + 0x7C) = (319UL << 16) | 319;              /* size_lcd */
    REG32(DISPC + 0x44) = (2UL << 1);                       /* config: load each frame */
    REG32(DISPC + 0x40) = (1UL << 3) | (3UL << 8);          /* control: TFT, 24-bit */
    REG32(DISPC + 0x4C) = 0;                                /* default_color0 */
    REG32(DISPC + 0x80) = VRAM_BASE;                        /* gfx_ba0 */
    REG32(DISPC + 0xA0) = 1 | (0x8UL << 1);                 /* gfx: enable + RGB24U */
    REG32(DISPC + 0xAC) = 1;                                /* gfx_row_inc */
    REG32(DISPC + 0xB0) = 1;                                /* gfx_pixel_inc */
    REG32(DISPC + 0x8C) = (319UL << 16) | 319;              /* gfx_size */
    REG32(DISPC + 0x40) |= (1UL << 0) | (1UL << 5);         /* LCD out + GO_LCD */
    return 0;
}

/* ------------------------------------------------------ steel renderer */

/* palette: j49-screen main.rs constants, 0xAARRGGBB little-endian ->
 * bytes B,G,R,X = the DISPC RGB24-unpacked scanout (and fbcon's XRGB8888) */
#define BG      0xFF0A0C10UL
#define SURF2   0xFF171B24UL
#define LINE    0xFF2A3040UL
#define DIM     0xFF6B7488UL
#define TEXT    0xFFD6DAE3UL
#define STRONG  0xFFF0F2F6UL
#define GOLD    0xFFF0B429UL
#define GOLD_L  0xFFFFD566UL
#define GREEN   0xFF45C16BUL
#define RED     0xFFE5484DUL
#define CYAN    0xFF3DBDC9UL
#define BLACK   0xFF000000UL

static void px(int x, int y, u32 c)
{
    if (x >= 0 && y >= 0 && x < FW && y < FH)
        VRAM[y * FW + x] = c;
}

static void rect(int x, int y, int w, int h, u32 c)
{
    int yy, xx;
    for (yy = y; yy < y + h; yy++)
        for (xx = x; xx < x + w; xx++)
            px(xx, yy, c);
}

/* 45-degree pixel stairs between two points (the steel vormentaal: skimming
 * diagonals are staircases of whole pixels, never anti-aliased strokes) */
static void stair_line(int x0, int y0, int x1, int y1, u32 c)
{
    int dx = x1 > x0 ? x1 - x0 : x0 - x1;
    int dy = y1 > y0 ? y1 - y0 : y0 - y1;
    int sx = x0 < x1 ? 1 : -1;
    int sy = y0 < y1 ? 1 : -1;
    int err = dx - dy;

    for (;;) {
        px(x0, y0, c);
        if (x0 == x1 && y0 == y1)
            break;
        int e2 = 2 * err;
        if (e2 > -dy) { err -= dy; x0 += sx; }
        if (e2 <  dx) { err += dx; y0 += sy; }
    }
}

/* midpoint circle: the frame rings (r149/150, the j49-screen frame_decor) */
static void circle(int r, u32 c)
{
    int x = 0, y = r, d = 1 - r;

    while (x <= y) {
        px(160 + x, 160 - y, c); px(160 + y, 160 - x, c);
        px(160 + y, 160 + x, c); px(160 + x, 160 + y, c);
        px(160 - x, 160 + y, c); px(160 - y, 160 + x, c);
        px(160 - y, 160 - x, c); px(160 - x, 160 - y, c);
        if (d < 0) {
            d += 2 * x + 3;
        } else {
            d += 2 * (x - y) + 5;
            y--;
        }
        x++;
    }
}

/* tick direction cos/sin in Q8: tick k at 6k degrees from 12 o'clock,
 * clockwise (generated table, see ../README.md) */
static const short tick_q8[60][2] = {
    {    0, -256},
    {   27, -255},
    {   53, -250},
    {   79, -243},
    {  104, -234},
    {  128, -222},
    {  150, -207},
    {  171, -190},
    {  190, -171},
    {  207, -150},
    {  222, -128},
    {  234, -104},
    {  243,  -79},
    {  250,  -53},
    {  255,  -27},
    {  256,    0},
    {  255,   27},
    {  250,   53},
    {  243,   79},
    {  234,  104},
    {  222,  128},
    {  207,  150},
    {  190,  171},
    {  171,  190},
    {  150,  207},
    {  128,  222},
    {  104,  234},
    {   79,  243},
    {   53,  250},
    {   27,  255},
    {    0,  256},
    {  -27,  255},
    {  -53,  250},
    {  -79,  243},
    { -104,  234},
    { -128,  222},
    { -150,  207},
    { -171,  190},
    { -190,  171},
    { -207,  150},
    { -222,  128},
    { -234,  104},
    { -243,   79},
    { -250,   53},
    { -255,   27},
    { -256,    0},
    { -255,  -27},
    { -250,  -53},
    { -243,  -79},
    { -234, -104},
    { -222, -128},
    { -207, -150},
    { -190, -171},
    { -171, -190},
    { -150, -207},
    { -128, -222},
    { -104, -234},
    {  -79, -243},
    {  -53, -250},
    {  -27, -255},
};

/* one tick: radial stroke r131..r141, every 5th reaching deeper (rail with
 * major marks -- maatstreepjes, from ui/steel.h) */
static void tick(int k, u32 c)
{
    int dx = tick_q8[k][0], dy = tick_q8[k][1];
    int r0 = 131, r1 = (k % 5 == 0) ? 141 : 138;

    stair_line(160 + (r0 * dx) / 256, 160 + (r0 * dy) / 256,
               160 + (r1 * dx) / 256, 160 + (r1 * dy) / 256, c);
}

static void round_mask(void)
{
    int x, y;
    for (y = 0; y < FH; y++)
        for (x = 0; x < FW; x++) {
            int dx = x - 160, dy = y - 160;
            if (dx * dx + dy * dy > 156 * 156)
                VRAM[y * FW + x] = BLACK;
        }
}

static int text_w(const char *s, int sc)
{
    int n = 0;
    while (*s++)
        n++;
    return n * 6 * sc - sc;
}

static void text(int x, int y, const char *s, int sc, u32 c)
{
    while (*s) {
        const u8 *g = glyph(*s++);
        int r, col;
        for (r = 0; r < 7; r++)
            for (col = 0; col < 5; col++)
                if (g[r] & (1 << (4 - col)))
                    rect(x + col * sc, y + r * sc, sc, sc, c);
        x += 6 * sc;
    }
}

static void text_center(int y, const char *s, int sc, u32 c)
{
    text(160 - text_w(s, sc) / 2, y, s, sc, c);
}

/* robo_panel: pixel-rounded plate with cut corners + bevels (j49-screen) */
static void plate(int x, int y, int w, int h, int ct, int cb)
{
    int xx, yy;
    for (yy = y; yy < y + h; yy++)
        for (xx = x; xx < x + w; xx++) {
            int lx = xx - x, ly = yy - y;
            int rx = w - 1 - lx, ry = h - 1 - ly;
            if (lx + ly < ct || rx + ly < ct || lx + ry < cb || rx + ry < cb)
                continue;
            px(xx, yy, SURF2);
        }
    /* hairline bevel: light top/left, dark bottom/right */
    for (xx = x + ct; xx < x + w - cb; xx++) {
        px(xx, y, DIM);
        px(xx, y + h - 1, BLACK);
    }
    for (yy = y + ct; yy < y + h - cb; yy++) {
        px(x, yy, DIM);
        px(x + w - 1, yy, BLACK);
    }
}

/* L-shaped gold corner brackets (j49-screen draw_brackets) */
static void brackets(int x, int y, int w, int h, int len)
{
    int i;
    for (i = 0; i < len; i++) {
        px(x + i, y, GOLD);               px(x, y + i, GOLD);
        px(x + w - 1 - i, y, GOLD);        px(x + w - 1, y + i, GOLD);
        px(x + i, y + h - 1, GOLD);        px(x, y + h - 1 - i, GOLD);
        px(x + w - 1 - i, y + h - 1, GOLD); px(x + w - 1, y + h - 1 - i, GOLD);
    }
}

/* hazard striping: only where something is wrong (homelab-ui .hazard) */
static void hazard(int x, int y, int w, int h)
{
    int xx, yy;
    for (yy = y; yy < y + h; yy++)
        for (xx = x; xx < x + w; xx++)
            px(xx, yy, ((((xx + yy) >> 2) & 1) == 0) ? GOLD : BLACK);
}

/* ------------------------------------------------------------ screens */

static const char *const phase_word[] = {
    "WAIT", "ERASE", "WRITE", "VERIFY", "DONE", "ERROR",
};
static const char *const err_word[] = {
    "", "E1 HEAD", "E2 BLOCK", "E3 DATA", "E4 NAND",
};
static int err_code;

static void ui_draw(int phase, int pct)
{
    u32 fg, track;
    int k, filled;
    char pctbuf[8];

    if (!panel_ok)
        return;

    rect(0, 0, FW, FH, BG);
    round_mask();
    circle(150, LINE);
    circle(149, LINE);

    /* header plate, like the j49-screen install screen */
    plate(92, 34, 136, 26, 3, 8);
    brackets(92, 34, 136, 26, 8);
    text_center(41, "INSTALLER", 2, GOLD);

    fg    = (phase == UI_DONE) ? GREEN : (phase == UI_ERR) ? RED : GOLD;
    track = (phase == UI_DONE) ? GREEN : (phase == UI_ERR) ? RED : LINE;

    filled = (pct * 60 + 50) / 100;
    if (phase == UI_WAIT) {
        fg = CYAN;
        filled = 0;
    }
    for (k = 0; k < 60; k++) {
        u32 c = track;
        if (k < filled)
            c = fg;
        tick(k, c);
    }
    if (filled > 0 && filled < 60 && phase != UI_DONE && phase != UI_ERR)
        tick(filled, GOLD_L);          /* the bright leading head */
    /* pointer ticks at 12/3/6/9 o'clock, like frame_decor */
    for (k = 0; k < 60; k += 15)
        tick(k, (phase == UI_WAIT) ? GOLD : GOLD_L);

    if (pct >= 100) {
        pctbuf[0] = '1'; pctbuf[1] = '0'; pctbuf[2] = '0'; pctbuf[3] = '%';
        pctbuf[4] = 0;
    } else if (pct >= 0) {
        pctbuf[0] = (char)('0' + pct / 10);
        pctbuf[1] = (char)('0' + pct % 10);
        pctbuf[2] = '%';
        pctbuf[3] = 0;
    } else {
        pctbuf[0] = '-'; pctbuf[1] = '-'; pctbuf[2] = '%'; pctbuf[3] = 0;
    }
    text_center(140, pctbuf, 4, (phase == UI_WAIT) ? DIM : fg);
    text_center(184, phase_word[phase], 2,
                phase == UI_DONE ? GREEN : phase == UI_ERR ? RED :
                phase == UI_WAIT ? CYAN : STRONG);

    /* target line: cyan = information (the stub writes spl0+spl1) */
    text_center(216, "SPL0+SPL1", 1, CYAN);
    text_center(230, "STOCK BCH4", 1, DIM);

    if (phase == UI_DONE)
        text_center(246, "HERSTART", 1, DIM);
    if (phase == UI_ERR) {
        text_center(246, err_word[err_code & 3 ? err_code & 3 : 1], 1, RED);
        hazard(86, 256, 148, 10);
    }
}

/* exported API */
void ui_phase(int phase, int pct)
{
    ui_draw(phase, pct);
}

void ui_err(int err)
{
    err_code = err;
    ui_draw(UI_ERR, -1);
}

void ui_phase_wait(void)
{
    ui_draw(UI_WAIT, -1);
}

/* ---------------------------------------------------------- bring-up */

void panel_init(void)
{
#ifdef NEST_NO_PANEL
    return;            /* blind install: no display bring-up at all */
#endif
    clocks_on();
    pads_on();
    clocks_up = 1;
    wdt_arm();               /* hang -> warm reset -> fresh ROM window */
    wdt_kick();
    beep(1);                 /* clocks + pads up */
    wait_ms(1);

    /* TWL4030: VAUX1 3.0V (LM3530 power), VAUX2 2.8V (panel VCC),
     * VPLL2 1.8V (vdds_dsi, the DSS needs it for DPI too) */
    if (twl_ldo(0x75, 0x04, 0x72))   /* VAUX1 */
        return;
    if (twl_ldo(0x79, 0x09, 0x76))   /* VAUX2 */
        return;
    if (twl_ldo(0x91, 0x05, 0x8E))   /* VPLL2 */
        return;
    beep(2);                 /* TWL LDOs ok */
    wait_ms(2);

    /* LM3530 backlight: HWEN high, GEN_CONFIG 0x1d (I2C mode, 29.5 mA),
     * brightness 108 -- the kernel DT values */
    if (i2c_init_bus(0x48060000UL))   /* I2C3 */
        return;
    gpio_out(GPIO6_BASE, 17, 1);
    wait_ms(2);
    if (i2c_write2(0x48060000UL, 0x36, 0x10, 0x1D) ||
        i2c_write2(0x48060000UL, 0x36, 0x20, 0x00) ||
        i2c_write2(0x48060000UL, 0x36, 0x30, 0x00) ||
        i2c_write2(0x48060000UL, 0x36, 0x41, 0x00) ||
        i2c_write2(0x48060000UL, 0x36, 0xA0, 108))
        return;
    beep(3);                 /* LM3530 ok */

    /* panel: SPI control path + DPI pixel path */
    if (i2c_init_bus(0x48070000UL))   /* I2C1 (TWL) -- already used, safe */
        return;
    if (spi_init())
        return;
    beep(4);                 /* McSPI ok */
    panel_reset();

    {
        u16 w[24];
        unsigned s;

        for (s = 0; s < sizeof(panel_seq) / sizeof(panel_seq[0]); s++) {
            int n = panel_seq[s].np + 1, j;
            w[0] = panel_seq[s].cmd;
            for (j = 0; j < panel_seq[s].np; j++)
                w[j + 1] = (u16)(0x100 | panel_seq[s].p[j]);
            spi_words(w, n);
        }
        wait_ms(120);                       /* SLPOUT ramp: 120 ms */
        for (s = 0; s < sizeof(panel_seq2) / sizeof(panel_seq2[0]); s++) {
            w[0] = panel_seq2[s].cmd;
            spi_words(w, 1);
            if (panel_seq2[s].cmd == 0x11)
                wait_ms(10);
        }
    }

    /* clear VRAM to the steel page, then start scanout */
    {
        u32 *p = VRAM, *end = VRAM + FW * FH;
        while (p < end)
            *p++ = BG;
    }
    if (dispc_init())
        return;        /* blind: bounded, never blocks the flash path */
    panel_ok = 1;
    beep(5);                 /* DISPC up, panel lit */

    ui_draw(UI_WAIT, -1);
    wait_ms(200);
}
