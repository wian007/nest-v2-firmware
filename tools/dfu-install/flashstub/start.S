/* start.S -- flashstub entry, linked at 0x80100000 (the slot the vendored
 * x-load receives u-boot.bin in and jumps to, see tools/omap-loader/push.sh).
 *
 * The jump from x-load lands here in an unknown cache/MMU state (x-load runs
 * physical, but nothing guarantees it switched its caches off), so make the
 * CPU state ours before any C runs: IRQs off, caches + MMU off, I-cache/TLB
 * invalidated, our own stack, BSS cleared. All stub code is integer-only
 * (the VFP coprocessor is not enabled -- see panel.c for why).
 */
.arm
.section .text.start, "ax"
.global _start
_start:
    cpsid   if

    /* SCTLR: I-cache, D-cache, MMU, alignment checking off */
    mrc     p15, 0, r0, c1, c0, 0
    bic     r0, r0, #0x1800
    bic     r0, r0, #0x0007
    mcr     p15, 0, r0, c1, c0, 0

    /* invalidate caches, branch predictor, TLBs */
    mov     r0, #0
    mcr     p15, 0, r0, c7, c5, 0
    mcr     p15, 0, r0, c7, c5, 6
    mcr     p15, 0, r0, c8, c7, 0
    dsb
    isb

    /* stack: 1 MB below the payload area, plenty for this stub */
    ldr     sp, =0x80300000

    /* clear .bss */
    ldr     r0, =__bss_start
    ldr     r1, =__bss_end
    mov     r2, #0
1:  cmp     r0, r1
    strlo   r2, [r0], #4
    blo     1b

    bl      main
hang:
    wfi
    b       hang

    /* modern binutils: mark the stack non-executable */
    .section .note.GNU-stack, "", %progbits
