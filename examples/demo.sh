#!/usr/bin/env bash
# Walkthrough: build a throwaway tree with one of every problem class and
# run the whole namefence CLI against it. Everything happens in a temp
# directory that is removed on exit — your own files are never touched.
set -euo pipefail

cd "$(dirname "$0")/.."
cargo build --quiet
BIN="$PWD/target/debug/namefence"

DEMO=$(mktemp -d "${TMPDIR:-/tmp}/namefence-demo.XXXXXX")
trap 'rm -rf "$DEMO"' EXIT

say() { printf '\n\033[1m$ %s\033[0m\n' "$*"; }

# --- a tree only Linux would tolerate -----------------------------------------
mkdir -p "$DEMO/docs" "$DEMO/photos" "$DEMO/junk"
echo x > "$DEMO/aux.txt"                            # reserved device name
echo x > "$DEMO/report:final.csv"                   # ':' is illegal on Windows
echo x > "$DEMO/notes. "                            # trailing space
echo x > "$DEMO/docs/README.md"
echo x > "$DEMO/docs/readme.md"                     # case twin
printf x > "$DEMO/photos/cafe$(printf '\xcc\x81').jpg"  # NFD, as a Mac stores it
printf x > "$DEMO/photos/caf$(printf '\xc3\xa9').jpg"   # NFC twin of the same text
echo x > "$DEMO/junk/Thumbs.db"                     # silently skipped by Dropbox
printf x > "$DEMO/junk/caf$(printf '\xe9').txt"     # Latin-1 bytes, not UTF-8

say "namefence check $DEMO"
"$BIN" check "$DEMO" || true

say "namefence check --targets cloud --format json $DEMO   (for CI pipelines)"
"$BIN" check --targets cloud --format json "$DEMO" || true

say "namefence explain NF007"
"$BIN" explain NF007

say "namefence fix $DEMO   (dry run — nothing is renamed)"
"$BIN" fix "$DEMO" || true

say "namefence fix --apply $DEMO"
"$BIN" fix --apply "$DEMO"

say "namefence check $DEMO   (after the fix)"
"$BIN" check "$DEMO" || true

say "git ls-files | namefence stdin   (lint a listing, no filesystem access)"
printf 'docs/Readme.md\ndocs/readme.md\nsrc/aux.rs\nok.txt\n' | "$BIN" stdin || true

printf '\nDemo tree was %s — removed on exit.\n' "$DEMO"
