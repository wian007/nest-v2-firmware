/* steel.h -- C mirror of the j49-screen steel theme, for the ONE renderer
 * that cannot run Rust: the bare-metal flashstub (session 1, before DRAM
 * Linux exists). Everything else (installer kernel sessions, OS boot) uses
 * tools/j49-screen directly -- in Rust, same binary, same screens.
 *
 * Source of truth: tools/j49-screen/src/main.rs
 *   - palette constants (BG..CYAN, GROND/MIDDEN/RANDC from achtergrond.frag)
 *   - Fb primitives (put/rect/disc/ring/robo_panel/brackets/glyph 5x7)
 *   - panel format: 320x320 XRGB8888, 0xAARRGGBB little-endian (B,G,R,X)
 * Upstream beyond that: homelab-ui frontend/src/index.css (:root tokens),
 * per its CLAUDE.md. When the palette changes there, change j49-screen
 * first, then mirror it here.
 *
 * Colour MEANS something (never decoration):
 *   gold  = look here (active phase, progress ring)
 *   green = done / good
 *   red   = alarm (verify fail, bad block, header mismatch)
 *   cyan  = information (target partition, address)
 *   everything else is greyscale.
 *
 * The Avatar bit: glow without alpha = dithered checkerboard rings around
 * the gold arc head, and the stepped vignette quantized exactly as
 * j49-screen's achtergrond.frag vignette() does it.
 */
#ifndef STEEL_H
#define STEEL_H

/* ------------------------------------------------------------ palette */
/* 0xAARRGGBB; little-endian emit = B,G,R,X bytes = the fb's XRGB8888. */
#define STEEL_BG     0xFF0A0C10 /* near-black page */
#define STEEL_SURF   0xFF10131A /* plate */
#define STEEL_SURF2  0xFF171B24 /* plate head / hover */
#define STEEL_LINE   0xFF2A3040 /* hairline */
#define STEEL_DIM    0xFF6B7488 /* muted text */
#define STEEL_TEXT   0xFFD6DAE3 /* text */
#define STEEL_STRONG 0xFFF0F2F6 /* emphasis */
#define STEEL_GOLD   0xFFF0B429 /* accent: look here */
#define STEEL_GOLD_L 0xFFFFD566 /* bright gold (arc head, streak heads) */
#define STEEL_GREEN  0xFF45C16B /* done */
#define STEEL_RED    0xFFE5484D /* alarm */
#define STEEL_CYAN   0xFF3DBDC9 /* info */
#define STEEL_BLACK  0xFF000000 /* outside the round mask */

/* Steel wall (achtergrond.frag): keep the stub simple -- flat BG plus the
 * frame ring; the groove grid + streaks stay j49-screen's, they need the
 * full renderer. The stub screen is deliberately quieter: one message. */

/* ------------------------------------------------------------- geometry */
/* Round panel 320x320; j49-screen masks at r=156, frame ring at r=149/150
 * with 10px gold ticks at N/E/S/W (frame_decor). The stub copies that. */
#define STEEL_W 320
#define STEEL_H 320
#define STEEL_CX 160
#define STEEL_CY 160
#define STEEL_R_MASK 156
#define STEEL_R_RING1 149
#define STEEL_R_RING2 150
#define STEEL_TICK_LEN 10

/* Progress ring: j49-screen Fb::ring() semantics -- annulus r_in=131,
 * r_out=141, sweep from 12 o'clock clockwise, lead frac in GOLD, track in
 * STEEL_LINE. Dithered glow: one checkerboard annulus just outside the
 * leading head (STEEL_MIT below), fading into the page. */
#define STEEL_RING_R_IN 131
#define STEEL_RING_R_OUT 141

/* Title plate + brackets (robo_panel + draw_brackets): y=44..70, w=176,
 * cut corners 3/8, gold brackets len 8. Phase word at scale 3 under it;
 * percentage scale 4 dead centre; target line (cyan) scale 1 at the bottom.
 * All text uppercase 5x7 glyphs, same glyph() table as j49-screen. */

/* --------------------------------------------------------------- phases */
/* Dutch, uppercase (the 5x7 font is uppercase-only). Mirror of the
 * j49-screen install screen's phase words -- keep both in sync. */
#define STEEL_PH_ERASE  "ERASE"
#define STEEL_PH_WRITE  "WRITE"
#define STEEL_PH_VERIFY "VERIFY"
#define STEEL_PH_DONE   "DONE"
#define STEEL_PH_ERR    "ERROR"

/* Error codes under the phase word (red): */
#define STEEL_E1 "E1 HEAD"      /* payload header magic/size mismatch */
#define STEEL_E2 "E2 BLOCK"     /* bad block beyond tolerance */
#define STEEL_E3 "E3 VERIFY" /* read-back != written */

#endif /* STEEL_H */
