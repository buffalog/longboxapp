#!/usr/bin/env bash
# Regenerate the SQLx offline-mode cache.
#
# Run this after any change to migrations/ or to a SQL query string in any
# crate. Commits that touch SQL must include the updated cache (under .sqlx/).
#
# Idempotent: nukes the throwaway DB each time.

set -euo pipefail

cd "$(dirname "$0")/.."

DB_FILE="./target/sqlx-prepare.db"
export DATABASE_URL="sqlite:${DB_FILE}"

mkdir -p ./target
rm -f "${DB_FILE}"

sqlx database create
sqlx migrate run --source longbox-db/migrations
cargo sqlx prepare --workspace -- --tests

rm -f "${DB_FILE}"

echo "SQLx offline cache regenerated under .sqlx/"
