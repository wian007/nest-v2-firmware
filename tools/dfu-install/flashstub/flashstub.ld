/*
 * flashstub.ld -- the stub replaces u-boot in the DFU chain: the vendored
 * x-load receives it at 0x80100000 (exactly where push.sh puts u-boot.bin)
 * and jumps there. Everything the stub needs must be at or above 0x80100000:
 * below it is nothing we own, above it are the payload (0x82000000, pushed
 * by the same x-load session) and the VRAM we pick (0x82800000).
 */
ENTRY(_start)

SECTIONS
{
    . = 0x80100000;

    .text : {
        start.o (.text.start)
        *(.text*)
        *(.rodata*)
    }

    .data : {
        *(.data*)
    }

    .bss : ALIGN(4) {
        __bss_start = .;
        *(.bss*)
        *(COMMON)
        __bss_end = .;
    }

    /DISCARD/ : { *(.ARM.exidx*) *(.ARM.attributes) *(.comment) }
}
