#!/bin/bash
# Fills ./assets with the exported KSP2 game assets the server needs. They are not in the package or on GitHub.
#   ./fetch-assets.sh                      download everything listed in assets-manifest.txt from ose.jfly.dev
#   ./fetch-assets.sh /path/to/assets/web  copy from a local export instead (e.g. a copy of the PC's assets\web)
# Re-running resumes: files already present at the listed size are skipped.
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
DEST="$HERE/assets"
SOURCE="${OSE_ASSETS_URL:-https://ose.jfly.dev/assets}"
mkdir -p "$DEST"
rm -f "$DEST/.complete"

if [ $# -ge 1 ]; then
  echo "Copying assets from $1 ..."
  rsync -a --exclude packages/ "${1%/}/" "$DEST/"
  touch "$DEST/.complete"
  echo "Assets ready."
  exit 0
fi

MANIFEST="$HERE/assets-manifest.txt"
total=$(wc -l < "$MANIFEST" | tr -d ' ')
echo "Downloading $total asset files (about $(awk -F'\t' '{s+=$1} END {printf "%.1f", s/1e9}' "$MANIFEST") GB) from $SOURCE"
echo "This happens once. Interrupt and re-run any time; it picks up where it stopped."

fetch_one() {
  size="$1"; path="$2"
  out="$DEST/$path"
  if [ -f "$out" ] && [ "$(stat -f %z "$out")" = "$size" ]; then return 0; fi
  mkdir -p "$(dirname "$out")"
  enc=$(printf '%s' "$path" | sed -e 's/%/%25/g' -e 's/ /%20/g' -e 's/#/%23/g' -e 's/?/%3F/g')
  if curl -fsSL --retry 5 --retry-all-errors --connect-timeout 20 -o "$out.part" "$SOURCE/$enc"; then
    mv "$out.part" "$out"
  else
    rm -f "$out.part"; echo "FAILED $path" >&2; return 1
  fi
}
export -f fetch_one
export DEST SOURCE

failed=0
tr '\t' '\n' < "$MANIFEST" | tr '\n' '\0' | xargs -0 -n 2 -P 8 bash -c 'fetch_one "$0" "$1"' || failed=1
have=$(find "$DEST" -type f ! -name '*.part' ! -name '.complete' | wc -l | tr -d ' ')
if [ "$failed" = 0 ]; then
  touch "$DEST/.complete"
  echo "Assets ready ($have files)."
else
  echo "Some downloads failed ($have of $total files present). Run ./fetch-assets.sh again to retry." >&2
  exit 1
fi
