#!/usr/bin/env bash
# Open an interactive sqlite3 shell against the catalog DB.
#
# The DB lives on the `longbox-db` Docker named volume (not a host
# bind mount — see docker-compose.yml for the why). This script
# spawns a throwaway alpine+sqlite container with that volume
# attached.
#
# Default mode is readonly because the live longbox container holds
# the WAL write lock; opening RW from a second process either fails
# (SQLITE_READONLY 8) or risks contention. Use --write only when the
# longbox container is stopped.
#
# Usage:
#   scripts/db-shell.sh                        # interactive readonly shell
#   scripts/db-shell.sh --write                # interactive RW shell (stop longbox first)
#   scripts/db-shell.sh -- 'SELECT 1;'         # one-shot readonly query
#   scripts/db-shell.sh --write -- 'VACUUM;'   # one-shot RW (stop longbox first)
set -euo pipefail

write=
one_shot=()
while (( $# )); do
  case "$1" in
    --write|-w) write=1; shift ;;
    --) shift; one_shot=("$@"); break ;;
    -h|--help) sed -n '2,/^set/p' "$0" | sed 's/^# \?//;/^set/d'; exit 0 ;;
    *) echo "unknown arg: $1" >&2; exit 2 ;;
  esac
done

mount_opts="longbox-db:/data"
sqlite_flags=(-cmd '.headers on' -cmd '.mode column')
if [[ -z $write ]]; then
  mount_opts="longbox-db:/data:ro"
  sqlite_flags+=(-readonly)
fi

docker_args=(--rm -v "$mount_opts")
(( ${#one_shot[@]} )) || docker_args+=(-it)

exec docker run "${docker_args[@]}" \
  keinos/sqlite3 \
  sqlite3 "${sqlite_flags[@]}" /data/longbox.db ${one_shot[@]+"${one_shot[@]}"}
