#!/usr/bin/env bash
# Bring a checkout of the public repository up to date with what is committed here: export
# (export-public.sh, with its checks) and replace the files of the public checkout. It does not
# commit or push: look at `git status` and `git diff` there first.
#
# Usage: tools/release/sync-public.sh <checkout of the public repository>
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
PUB="${1:?usage: sync-public.sh <checkout of the public repository>}"
[ -d "$PUB/.git" ] || { echo "$PUB is not a git checkout" >&2; exit 1; }
TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT
"$HERE/export-public.sh" "$TMP/tree"
rsync -a --delete --exclude .git "$TMP/tree/" "$PUB/"
git -C "$PUB" status --short | head -n 40
