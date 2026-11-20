#!/bin/bash
# OpenSpaceExplorer on a Mac: fetches the game assets on first run, starts the server, opens the browser.
# Run it from Terminal (`bash start.command`) the first time; after that a double-click works too.
# Environment: OSE_PORT (default 7777), OSE_DATA (default ~/Library/Application Support/OpenSpaceExplorer).
set -euo pipefail
HERE="$(cd "$(dirname "$0")" && pwd)"
PORT="${OSE_PORT:-7777}"
DATA="${OSE_DATA:-$HOME/Library/Application Support/OpenSpaceExplorer}"
URL="http://localhost:$PORT/"

# A browser download marks every file as quarantined; Gatekeeper would then refuse the unsigned server.
xattr -dr com.apple.quarantine "$HERE" 2>/dev/null || true
# Apple Silicon only runs signed code; re-sign ad hoc if the signature did not survive the copy.
codesign -v "$HERE/ose-server" 2>/dev/null || codesign --force -s - "$HERE/ose-server"

if [ ! -f "$HERE/assets/.complete" ]; then
  "$HERE/fetch-assets.sh"
fi

if curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then
  echo "Something is already answering on port $PORT; opening $URL"
  open "$URL"
  exit 0
fi

mkdir -p "$DATA"
cd "$DATA"   # accounts, saves, room files and replays live here and survive package updates
echo "Starting OpenSpaceExplorer on $URL (Ctrl+C stops it; data in $DATA)"
(
  for _ in $(seq 1 180); do
    if curl -fsS "http://127.0.0.1:$PORT/health" >/dev/null 2>&1; then open "$URL"; exit 0; fi
    sleep 1
  done
  echo "The server did not answer within 3 minutes; see the log above." >&2
) &
exec "$HERE/ose-server" \
  --listen "127.0.0.1:$PORT" \
  --static-dir "$HERE/web" \
  --assets-dir "$HERE/assets" \
  --db "$DATA/ose-accounts.json" \
  --save "$DATA/ose-save.json" \
  --allow-guest-accounts \
  --guest-ttl-s 0
