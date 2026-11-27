#!/usr/bin/env bash
# make-installer-dtb.sh <kernel-src> <kernel-out> <out.dtb> [installer|a-slot]
#
# The installer kernel boots the same DTS as production with two differences
# (everything else stays byte-identical):
#   1. root0 and boot0 lose `read-only`: stage 2 writes root0, stage 3 writes boot0.
#      ipl/spl0/spl1 stay read-only (nothing in this flow may touch them).
#   2. a `staging@82000000` reservation (22 MB): the raw root0 jffs2 that stage 2
#      downloads into DRAM over DFU. Without it the kernel treats the window as free
#      RAM and overwrites it while unpacking the initramfs (seen 2026-11-23: magic 0x0000).
#      Reserved like ramring, NOT no-map, so /dev/mem read() still works.
set -euo pipefail
KSRC="${1:?kernel source}"; KOUT="${2:?kernel out dir (needs scripts/dtc/dtc)}"; OUT="${3:?out.dtb}"
MODE="${4:-installer}"   # installer: boot0+root0 writable + staging; a-slot: only root0 writable (production kernel booting root0)
DTSDIR="$KSRC/arch/arm/boot/dts/ti/omap"
DTS="$DTSDIR/omap3-nest-j49.dts"
[ -f "$DTS" ] || { echo "missing $DTS" >&2; exit 1; }
DTC="$KOUT/scripts/dtc/dtc"; [ -x "$DTC" ] || DTC="$(command -v dtc)"
TMP="$(mktemp -d "${TMPDIR:-/tmp}/nest-instdtb.XXXXXX")"; trap 'rm -rf "$TMP"' EXIT
python3 -I - "$DTS" "$TMP/inst.dts" "$MODE" <<'PY'
import re, sys
s = open(sys.argv[1]).read()
mode = sys.argv[3]
for name in (("boot0@400000", "root0@c00000") if mode == "installer" else ("root0@c00000",)):
    s, n = re.subn(r'(%s\s*\{[^}]*?);?\s*read-only;(\s*\};)' % re.escape(name),
                   lambda m: m.group(1) + ';' + m.group(2), s)
    assert n == 1, f"{name}: read-only not found exactly once ({n})"
anchor = "\t\tramoops@83e00000 {"
assert s.count(anchor) == 1, "ramoops anchor not found"
node = "" if mode != "installer" else ("\t\t/* INSTALLER ONLY: DFU staging window for the raw root0 jffs2 (see make-installer-dtb.sh) */\n"
        "\t\tstaging@82000000 {\n\t\t\treg = <0x82000000 0x01600000>;\n\t\t};\n\n")
s = s.replace(anchor, node + anchor, 1)
open(sys.argv[2], "w").write(s)
PY
CPP=${CPP:-cpp}
cp "$TMP/inst.dts" "$DTSDIR/_nest_inst.dts"
trap 'rm -f "$DTSDIR/_nest_inst.dts"; rm -rf "$TMP"' EXIT
$CPP -nostdinc -I "$KSRC/include" -I "$KSRC/arch/arm/boot/dts" -I "$DTSDIR" -undef -D__DTS__ \
     -x assembler-with-cpp "$DTSDIR/_nest_inst.dts" > "$TMP/inst.pp.dts"
"$DTC" -I dts -O dtb -i "$DTSDIR" -i "$KSRC/arch/arm/boot/dts" -o "$OUT" "$TMP/inst.pp.dts" 2>/dev/null
ls -l "$OUT"
