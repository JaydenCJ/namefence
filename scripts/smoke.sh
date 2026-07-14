#!/usr/bin/env bash
# Smoke test: builds namefence, lints a synthetic tree containing one of
# every problem class end to end (text + JSON + exit codes), verifies the
# fix planner and --apply including collision-safe numbering, exercises the
# stdin mode, and finally lints namefence's own tree. Self-contained: temp
# dirs only, no network, idempotent. Prints "SMOKE OK" on success.
set -euo pipefail

cd "$(dirname "$0")/.."

fail() { echo "SMOKE FAIL: $*" >&2; exit 1; }

echo "[smoke] building..."
cargo build --quiet
BIN="$PWD/target/debug/namefence"

WORK=$(mktemp -d "${TMPDIR:-/tmp}/namefence-smoke.XXXXXX")
trap 'rm -rf "$WORK"' EXIT

# --- 1. version/help/checks/explain sanity -----------------------------------
"$BIN" --version | grep -q '^namefence 0\.1\.0$' || fail "--version mismatch"
"$BIN" --help | grep -q 'COMMANDS:' || fail "--help missing sections"
"$BIN" checks | grep -c '^NF0' | grep -qx '13' || fail "checks must list 13 checks"
"$BIN" explain NF007 | grep -q 'Syncthing and Dropbox' || fail "explain NF007 missing the story"
if "$BIN" explain NF042 2>/dev/null; then fail "unknown check accepted"; fi
echo "[smoke] version/help/checks/explain OK"

# --- 2. a tree with one of everything -----------------------------------------
FIX="$WORK/tree"
mkdir -p "$FIX/docs" "$FIX/photos" "$FIX/junk"
NFD_NAME="cafe$(printf '\xcc\x81').jpg"        # e + COMBINING ACUTE (as macOS stores it)
NFC_NAME="caf$(printf '\xc3\xa9').jpg"         # precomposed U+00E9
echo x > "$FIX/aux.txt"                        # NF001 reserved stem
echo x > "$FIX/report:final.csv"               # NF002 illegal char
printf x > "$FIX/$(printf 'line\nbreak').log"  # NF003 control char (real newline)
echo x > "$FIX/notes. "                        # NF004 trailing space
echo x > "$FIX/ draft.md"                      # NF005 leading space
echo x > "$FIX/docs/README.md"                 # kept
echo x > "$FIX/docs/readme.md"                 # NF006 case twin
printf x > "$FIX/photos/$NFD_NAME"             # NF008 non-NFC name...
printf x > "$FIX/photos/$NFC_NAME"             # ...NF007-colliding with this one
echo x > "$FIX/junk/Thumbs.db"                 # NF012 cloud reserved (no auto-fix)
printf x > "$FIX/junk/caf$(printf '\xe9').txt" # NF013 invalid UTF-8 (Latin-1)

echo "[smoke] namefence check (expect findings, exit 1)"
if "$BIN" check "$FIX" > "$WORK/check.out"; then fail "findings must exit 1"; fi
for want in \
  'aux.txt: error NF001 (windows-reserved-name)' \
  'fix: rename to `aux_.txt`' \
  'report:final.csv: error NF002 (windows-illegal-char)' \
  'error NF003 (control-character)' \
  'notes. : error NF004 (trailing-dot-or-space)' \
  ' draft.md: warning NF005 (leading-space)' \
  'docs/readme.md: error NF006 (case-collision)' \
  'collides with sibling `README.md`' \
  'error NF007 (normalization-collision)' \
  'warning NF008 (non-nfc)' \
  'junk/Thumbs.db: warning NF012 (cloud-reserved-name)' \
  'error NF013 (invalid-utf8)' \
  '11 file(s), 3 directory(ies) scanned'; do
  grep -qF "$want" "$WORK/check.out" || fail "check output missing: $want"
done
echo "[smoke] check findings OK"

# --- 3. exit-code policy, selection and JSON for CI ---------------------------
"$BIN" check --fail-on never "$FIX" > /dev/null || fail "--fail-on never must exit 0"
if "$BIN" check --targets cloud "$FIX" > /dev/null; then fail "cloud target still has findings"; fi
"$BIN" check --only NF012 --fail-on error "$FIX" > /dev/null \
  || fail "NF012 is a warning; --fail-on error must exit 0"
"$BIN" check --targets linux "$FIX" | grep -q 'findings: none' \
  || fail "--targets linux must mute everything in this tree"
"$BIN" check --format json "$FIX" > "$WORK/check.json" || true
grep -q '"tool": "namefence"' "$WORK/check.json" || fail "JSON missing tool field"
grep -q '"check": "NF001", "name": "windows-reserved-name", "severity": "error"' "$WORK/check.json" \
  || fail "JSON missing NF001 finding"
grep -q '"fix": "aux_.txt"' "$WORK/check.json" || fail "JSON missing fix suggestion"
grep -q '"fix": null' "$WORK/check.json" || fail "JSON advisory findings must have null fix"
grep -q '"truncated": false' "$WORK/check.json" || fail "JSON stats missing"
echo "[smoke] exit codes + selection + JSON OK"

# --- 4. fix: plan is a dry run, --apply converges ------------------------------
if "$BIN" fix "$FIX" > "$WORK/plan.out"; then fail "unapplied plan must exit 1"; fi
grep -qF 'would rename `docs/readme.md` -> `readme-2.md`' "$WORK/plan.out" \
  || fail "plan missing collision-safe numbering"
grep -qF 'would rename `aux.txt` -> `aux_.txt`  (NF001)' "$WORK/plan.out" \
  || fail "plan missing reserved-name rename"
[ -e "$FIX/aux.txt" ] || fail "plan must not touch the tree"
"$BIN" fix --apply "$FIX" > "$WORK/apply.out" || fail "apply failed"
grep -q 'applied .* rename(s)' "$WORK/apply.out" || fail "apply summary missing"
[ -e "$FIX/aux_.txt" ] || fail "aux.txt was not renamed"
[ -e "$FIX/docs/readme-2.md" ] || fail "case twin was not numbered"
# The NFD photo normalizes to exactly its NFC sibling's name — the planner
# must have bumped the rename to `-2` instead of clobbering the sibling.
[ -e "$FIX/photos/caf$(printf '\xc3\xa9')-2.jpg" ] || fail "NFC rename was not deduped to -2"
[ -e "$FIX/photos/$NFC_NAME" ] || fail "the clean NFC sibling must be untouched"
# Only the advisory Thumbs.db warning may remain; errors must be gone.
"$BIN" check --fail-on error "$FIX" > /dev/null || fail "errors remain after --apply"
("$BIN" check "$FIX" || true) | grep -q 'NF012' || fail "advisory NF012 should survive (no auto-fix)"
"$BIN" fix "$FIX" | grep -q 'nothing to fix' || fail "second fix must be a no-op"
echo "[smoke] fix plan + apply + convergence OK"

# --- 5. stdin mode: lint a git listing without touching the disk --------------
printf 'docs/Readme.md\ndocs/readme.md\nsrc/aux.rs\nok.txt\n' | "$BIN" stdin > "$WORK/stdin.out" || true
grep -q 'docs/readme.md: error NF006' "$WORK/stdin.out" || fail "stdin missed the case twin"
grep -q 'src/aux.rs: error NF001' "$WORK/stdin.out" || fail "stdin missed the reserved name"
printf 'a\nb.txt\0ok.txt\0' | "$BIN" stdin -0 > "$WORK/stdin0.out" || true
grep -q 'NF003' "$WORK/stdin0.out" || fail "-0 must preserve newline bytes in names"
echo "[smoke] stdin OK"

# --- 6. dogfood: namefence's own tree must be portable -------------------------
"$BIN" check . > "$WORK/self.out" || fail "namefence's own tree has findings: $(cat "$WORK/self.out")"
grep -q 'findings: none' "$WORK/self.out" || fail "self-check summary unexpected"
echo "[smoke] self-check OK"

echo "SMOKE OK"
