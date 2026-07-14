# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-07-12

### Added

- Check catalog NF001–NF013: Windows reserved device names (with any extension, including the superscript COM¹/LPT¹ variants), Windows-forbidden characters, ASCII control characters, trailing dots/spaces, leading spaces, case collisions, Unicode normalization collisions, non-NFC names, invisible/bidi/lookalike-space characters, component length (255 UTF-8 bytes *and* 255 UTF-16 units), path length budget, cloud-client reserved names, and invalid UTF-8.
- From-scratch canonical Unicode normalization (NFC/NFD per UAX #15) in pure std Rust: full canonical decomposition, canonical reordering, composition with blocking, algorithmic Hangul, driven by tables generated from UnicodeData.txt (UCD 14.0).
- Case-collision detection using canonical normalization plus full Unicode case folding, matching how NTFS and APFS actually compare names.
- Per-finding fix suggestions, and a `fix` command that merges them into one collision-safe rename plan: suggested names are guaranteed not to case- or normalization-collide with kept siblings or other suggestions (numbered `-2`, `-3` bumps), applied deepest-first, refusing to overwrite.
- `check` command with `--format text|json`, `--fail-on error|warning|info|never`, `--only`/`--skip` check selection, `--targets windows,macos,linux,cloud` platform profiles, `--max-path` and `--max-files` budgets.
- `stdin` command linting newline- or NUL-separated path lists (`git ls-files -z | namefence stdin -0`) with cross-path collision detection, without touching the filesystem.
- `checks` and `explain` commands documenting every rule with its rationale and fix recipe.
- Deterministic walker: byte-sorted traversal, `.git` contents skipped, symlinks never followed, honest truncation reporting at `--max-files`.
- Non-UTF-8 filename support end to end on Unix (detection, display escaping, lossy fix suggestion).
- Test suite: 81 unit tests, 17 CLI integration tests, and `scripts/smoke.sh`.

[0.1.0]: https://github.com/JaydenCJ/namefence/releases/tag/v0.1.0
