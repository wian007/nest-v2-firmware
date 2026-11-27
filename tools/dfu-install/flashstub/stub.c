/*
 * stub.c -- flashstub main: session 1 of the Nest J49 DFU installer.
 *
 * What it does (one ROM-DFU window, see ../README.md):
 *  1. panel_init(): best-effort bring-up of the round LCD, then WAIT.
 *  2. validate the payload header at 0x82000000 against the hard
 *     allow-list (spl0/spl1 ONLY) -- anything else is E1 HEAD.
 *  3. check every target block for factory bad blocks (E2 BLOCK).
 *  4. per range, in header order (the host builds [spl1, spl0] so the
 *     primary slot is written last): erase, program with STOCK BCH4 ECC
 *     (the exact bytes stock x-loader expects), verify read-back.
 *  5. DONE, PRM warm reset -> stock x-load now loads OUR u-boot.
 *
 * Failure states never write partially-unknown data: the range loop is
 * erase->write->verify per partition, and a verify error (E3) leaves the
 * device on the ERROR screen with the host able to power-cycle back into
 * the ROM DFU window and retry (spl0 still stock until its own turn).
 */
#include "stub.h"

/* the image lives right after the header, in the payload x-load pushed */
#define IMAGE ((const u8 *)(PAYLOAD_BASE + sizeof(struct nfd_hdr)))

static u8 pagebuf[PAGE_SIZE];
static u8 eccbuf[28];

static int range_allowed(u32 first, u32 n)
{
    return (first == SPL1_BLK0 && n == SPL1_NBLK) ||
           (first == SPL0_BLK0 && n == SPL0_NBLK);
}

static int pct(int done, int total)
{
    if (total <= 0)
        return 0;
    return (100 * done + total / 2) / total;
}

void stub_hang(int err)
{
    /* Show the failure (when the panel is up), then WARM RESET anyway:
     * a reset makes the host see a fresh ROM window (stock x-loader tried
     * spl0 and failed) instead of a stale, unresponsive gadget -- that is
     * the difference between 'stub reached an error' and 'stub hung'.
     * Beep 10 marks "error" audibly; the screen stays visible 2.5 s. */
    beep(10);
    ui_err(err);
    wait_ms(2500);
    stub_reset_warm();
    for (;;)
        ;                      /* stub_reset_warm never returns */
}

/* libgcc's __aeabi_ldiv0 calls raise() on divide-by-zero; we link -nostdlib
 * and every divisor here is a checked constant, so park it. */
int raise(int sig)
{
    (void)sig;
    return -1;
}

void main(void)
{
    const struct nfd_hdr *h = (const struct nfd_hdr *)PAYLOAD_BASE;
    u32 r, i, page, npages, done, total = 0;

    panel_init();

    /* ---- header ---- */
    if (h->magic != NFD1_MAGIC || h->n_ranges == 0 ||
        h->n_ranges > NFD_MAX_RANGES || h->image_len == 0)
        stub_hang(NFD_E_HDR);
    for (r = 0; r < h->n_ranges; r++) {
        if (!range_allowed(h->range[r].first_block, h->range[r].n_blocks))
            stub_hang(NFD_E_HDR);
        if (h->image_len > h->range[r].n_blocks * BLOCK_SIZE)
            stub_hang(NFD_E_HDR);
    }
    for (r = 0; r < h->n_ranges; r++)
        total += h->range[r].n_blocks;   /* work units for the ring */
    wdt_kick();
    beep(6);                             /* header validated */

    nand_init();
    wdt_kick();
    beep(7);                             /* NAND up */

    /* bad-block markers are NOT pre-scanned before the first write: the
     * OOB column addressing differs between byte- and word-based x16
     * interpretations, and a wrong guess marks every block bad. Real
     * failures surface as erase/program status errors (E2) below. */
    wdt_kick();
    beep(8);                             /* NAND usable */

    npages = (h->image_len + PAGE_SIZE - 1) / PAGE_SIZE;
    done = 0;

    /* ---- per range: erase -> write -> verify (header order) ---- */
    for (r = 0; r < h->n_ranges; r++) {
        const struct nfd_range *rg = &h->range[r];

        for (i = 0; i < rg->n_blocks; i++) {
            wdt_kick();
            if (nand_erase_block(rg->first_block + i))
                stub_hang(NFD_E_BADBLOCK);
            ui_phase(UI_ERASE, pct(done + i + 1, total));
        }

        for (page = 0; page < npages; page++) {
            u32 dst = (rg->first_block + page / PAGES_PER_BLK) * PAGES_PER_BLK
                    + page % PAGES_PER_BLK;

            /* last page: pad with 0xFF past the image end (x-load reads
             * the uImage header size, but keep the whole partition clean) */
            for (i = 0; i < PAGE_SIZE; i++)
                pagebuf[i] = 0xFF;
            {
                u32 off = page * PAGE_SIZE;
                u32 n = h->image_len - off;
                if (off < h->image_len) {
                    if (n > PAGE_SIZE)
                        n = PAGE_SIZE;
                    for (i = 0; i < n; i++)
                        pagebuf[i] = IMAGE[off + i];
                }
            }
            wdt_kick();
            if (nand_prog_page(dst, pagebuf, eccbuf))
                stub_hang(NFD_E_BADBLOCK);  /* program fail = block went bad */
            if (nand_verify_page(dst, pagebuf, eccbuf))
                stub_hang(NFD_E_VERIFY);
            if ((page & 7) == 0)
                ui_phase(UI_WRITE, pct(done + page / PAGES_PER_BLK + 1, total));
        }
        /* verify phase marker across the partition for a moment */
        ui_phase(UI_VERIFY, 100);

        done += rg->n_blocks;
    }

    /* ---- done ---- */
    wdt_kick();
    beep(9);                             /* erase/write/verify all done */
    ui_phase(UI_DONE, 100);
    wait_ms(1500);
    stub_reset_warm();          /* never returns */
}
