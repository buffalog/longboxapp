#!/bin/sh
# Fail when a change adds an env file or a 64-hex token (API keys such as
# ComicVine's or Metron's). Only ADDED lines are scanned, so existing code
# never trips it.
#
#   scripts/check-secrets.sh --staged        # pre-commit (scripts/git-hooks)
#   scripts/check-secrets.sh --range BASE    # CI: everything BASE..HEAD adds
#
# 64-hex strings that are not secrets live in Cargo.lock / pnpm-lock.yaml
# (checksums) and .sqlx/ (query hashes); those paths are skipped.
set -eu

case "${1:-}" in
  --staged) diff="git diff --cached" ;;
  --range) diff="git diff ${2:?--range needs a base}...HEAD" ;;
  *) echo "usage: $0 --staged | --range BASE" >&2; exit 2 ;;
esac

status=0

# Env files: *.env and .env.*, except the committed .env.example.
envs=$($diff --name-only --diff-filter=AR | grep -E '(^|/)([^/]*\.env|\.env\.[^/]*)$' | grep -vE '(^|/)\.env\.example$' || true)
if [ -n "$envs" ]; then
  echo "check-secrets: env file(s) in this change:" >&2
  echo "$envs" | sed 's/^/  /' >&2
  status=1
fi

# 64-hex tokens on added lines, outside the paths that legitimately carry hashes.
hits=$($diff -U0 --diff-filter=AM -- . ':(exclude)Cargo.lock' ':(exclude)**/pnpm-lock.yaml' ':(exclude).sqlx/**' ':(exclude)**/.sqlx/**' \
  | awk '/^\+\+\+ b\// { file = substr($0, 7); next } /^\+/ && /(^|[^0-9a-fA-F])[0-9a-fA-F]{64}([^0-9a-fA-F]|$)/ { print file }' \
  | sort -u)
if [ -n "$hits" ]; then
  echo "check-secrets: 64-hex token(s) added in:" >&2
  echo "$hits" | sed 's/^/  /' >&2
  status=1
fi

exit $status
