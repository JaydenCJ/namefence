# The namefence check catalog

Thirteen checks, NF001–NF013. IDs are stable across releases: new checks
append, existing IDs are never renumbered or reused. `namefence explain <ID>`
prints the long story for any of them; this document adds the engine-fidelity
notes and known deviations.

## Catalog

| ID | Name | Severity | Breaks on | Auto-fix |
|---|---|---|---|---|
| NF001 | windows-reserved-name | error | windows, cloud | append `_` to the stem |
| NF002 | windows-illegal-char | error | windows, cloud | per-character substitution |
| NF003 | control-character | error | windows, cloud | remove |
| NF004 | trailing-dot-or-space | error | windows, cloud | trim (fallback `_`) |
| NF005 | leading-space | warning | windows, cloud | trim |
| NF006 | case-collision | error | windows, macos, cloud | numbered rename of later names |
| NF007 | normalization-collision | error | macos, cloud | numbered rename of later names |
| NF008 | non-nfc | warning | macos, cloud | re-encode to NFC |
| NF009 | invisible-character | warning | all four | remove / replace with plain space |
| NF010 | component-too-long | error | all four | truncate stem, keep extension |
| NF011 | path-too-long | warning | windows, cloud | none (shorten directories by hand) |
| NF012 | cloud-reserved-name | warning | cloud | none (usually litter to delete) |
| NF013 | invalid-utf8 | error | windows, macos, cloud | replace invalid bytes with `_` |

Severities: `error` = the name is unusable or destructive somewhere;
`warning` = the name works but will bite you (silent skip, phantom diff,
human confusion). `--fail-on` converts either into an exit-code policy.

## Semantics worth knowing

- **NF001** compares the stem before the first dot, case-insensitively,
  after stripping trailing spaces and dots the way the Win32 layer does — so
  `aux.tar.gz`, `NuL.txt` and `nul .txt` are all flagged. `COM10` is not
  reserved and not flagged. The superscript variants `COM¹`/`LPT²` from the
  current Microsoft naming rules are included.
- **NF006** and **NF007** are per-directory checks. The first name of a
  colliding group (in byte order) is treated as canonical; every later
  member gets the finding, naming the sibling it collides with. A pair that
  is NFC-equal reports NF007 only — the more precise diagnosis — while NF006
  covers pairs that additionally need case folding to collide.
- **NF009** deliberately does *not* flag U+3000 IDEOGRAPHIC SPACE: it is a
  visible, intentionally-typed wide space in CJK filenames, not an artifact.
- **NF010** enforces both budgets at once: 255 UTF-8 bytes (ext4 and
  friends) and 255 UTF-16 code units (NTFS; macOS enforces the same count at
  the API layer). A 90-character CJK name passes UTF-16 but fails bytes; a
  200-emoji name fails both.
- **NF011** measures the path relative to the scan root in UTF-16 units
  against a conservative default budget of 240 (`--max-path` to change),
  because the invisible absolute prefix on the other machine eats the rest
  of MAX_PATH = 260. Only the shallowest offender is reported — descendants
  of an over-budget directory are implied, not spammed.
- **NF013** can only occur on platforms with byte-string filenames (Linux);
  on such platforms namefence reads raw `OsStr` bytes, so detection, display
  escaping and the lossy fix all work without any lossy pre-conversion.

## Normalization engine fidelity

namefence implements canonical normalization (UAX #15) from scratch in std
Rust: full recursive canonical decomposition, canonical reordering by
combining class, composition with the standard blocking rule, and
algorithmic Hangul — driven by tables generated from `UnicodeData.txt`
(UCD 14.0), with composition exclusions filtered by recomposition
round-trip. Singleton decompositions (U+212B ANGSTROM SIGN → U+00C5,
CJK compatibility ideographs, and the rest) are included.

Known, deliberate limitations:

- **Compatibility (NFKC/NFKD) equivalence is out of scope.** Filesystems
  never apply it, so `ﬁle` (with a ligature) and `file` are honestly
  different names everywhere and are not reported as colliding.
- **Case folding** uses `char::to_lowercase` after NFC — the full Unicode
  simple+special lowercase mapping shipped with Rust. NTFS's actual
  comparison uses a per-volume `$UpCase` table and APFS uses its own folding;
  the differences are confined to exotic code points and both directions of
  disagreement are harmless here (a rare false negative, never data loss).
- The **UCD snapshot is 14.0**; code points assigned in later Unicode
  versions normalize as themselves. Regenerating the tables from a newer
  UnicodeData.txt is a data-only change.

## Exit codes

| Code | Meaning |
|---|---|
| 0 | no findings at or above `--fail-on` (default `warning`) |
| 1 | findings at or above `--fail-on`, or a non-empty unapplied `fix` plan |
| 2 | usage error or I/O error |
