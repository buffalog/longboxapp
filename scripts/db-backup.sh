#!/usr/bin/env bash
# Snapshot the catalog DB from the `longbox-db` named volume to a
# host-side file. Uses sqlite3's `.backup` command (consistent online
# snapshot — safe to run while the longbox container is live).
#
# Usage:
#   scripts/db-backup.sh                       # → ./longbox-backup-YYYYMMDD-HHMMSS.db
#   scripts/db-backup.sh /path/to/dest.db      # explicit destination
set -euo pipefail

dest="${1:-./longbox-backup-$(date +%Y%m%d-%H%M%S).db}"
dest_abs="$(cd "$(dirname "$dest")" && pwd)/$(basename "$dest")"
dest_dir="$(dirname "$dest_abs")"

docker run --rm \
  -v longbox-db:/src:ro \
  -v "$dest_dir":/dest \
  keinos/sqlite3 \
  sqlite3 /src/longbox.db ".backup '/dest/$(basename "$dest_abs")'"

ls -lh "$dest_abs"
echo "Backed up to $dest_abs"
