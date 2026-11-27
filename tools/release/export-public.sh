#!/usr/bin/env bash
# Export the public tree: what is committed (HEAD), without the parts that stay private or
# are only notes from the bring-up, into an empty directory. Then check the result for
# things that must not be published. The public repository is made from that directory; it
# has its own history.
#
# Usage: tools/release/export-public.sh <empty target directory>
set -euo pipefail
HERE="$(cd "$(dirname "$0")/../.." && pwd)"
OUT="${1:?usage: export-public.sh <empty target directory>}"
mkdir -p "$OUT"
[ -z "$(ls -A "$OUT" | grep -v '^\.git$' || true)" ] || { echo "$OUT is not empty" >&2; exit 1; }

# Paths that are not exported.
EXCLUDE=(
    tools/night-check.sh             # checks one home network
    tools/j49-ui                     # desktop emulator: its own repository
    kas/nest-j49-robot.yml           # the private image with the robot screen
)
args=()
for e in "${EXCLUDE[@]}"; do args+=(":(exclude)$e"); done
git -C "$HERE" archive --format=tar HEAD -- . "${args[@]}" | tar -x -C "$OUT"

# Nothing of this may be in a public tree. Patterns of private data are in a file outside
# the repository when you have more (one extended regular expression per line).
bad=0
PATTERNS='192\.168\.(2|66)\.|/home/[a-z]+/|/mnt/backups|@gmail\.com'
EXTRA="${J49_PRIVATE_PATTERNS:-$HOME/.config/j49/private-patterns}"
[ -f "$EXTRA" ] && PATTERNS="$PATTERNS|$(grep -v '^#' "$EXTRA" | grep . | paste -sd'|')"
if grep -rInE "$PATTERNS" "$OUT" --exclude-dir=.git | grep -v 'Copyright\|tools/omap-loader/omap_loader.c\|tools/release/export-public.sh'; then
    echo "^ private data in the export" >&2; bad=1
fi
# a large file is a mistake, except the two kernels the installer uses
big=$(find "$OUT" -path "$OUT/.git" -prune -o -type f -size +1M -print | grep -v -e '/tools/dfu-install/payload/nest-service-uImage$' -e '/tools/dfu-install/installer/out/uImage-j49-a$' || true)
[ -z "$big" ] || { echo "large files in the export:"; echo "$big"; bad=1; }
[ -f "$OUT/LICENSE" ] && [ -f "$OUT/DISCLAIMER.md" ] || { echo "LICENSE or DISCLAIMER.md is missing" >&2; bad=1; }
echo "exported $(find "$OUT" -path "$OUT/.git" -prune -o -type f -print | wc -l) files to $OUT"
exit $bad
