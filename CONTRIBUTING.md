# Contributing to namefence

Thanks for your interest in improving namefence. Issues, discussions and pull requests are all welcome.

## Getting started

Prerequisites: Rust 1.75 or newer (stable toolchain).

```bash
git clone https://github.com/JaydenCJ/namefence.git
cd namefence
cargo build
cargo test
bash scripts/smoke.sh
```

`scripts/smoke.sh` builds the binary and runs it end to end against a synthetic tree containing one of every problem class — check, fix, --apply, stdin mode, JSON and exit codes. It finishes in well under a minute and must print `SMOKE OK`.

## Before you open a pull request

1. `cargo fmt` — formatting is enforced.
2. `cargo clippy --all-targets -- -D warnings` — clippy must be clean.
3. `cargo test` — unit tests and the CLI integration tests must pass.
4. `bash scripts/smoke.sh` — the smoke test must print `SMOKE OK`.
5. Add tests for behavior changes. All detection and fix logic lives in pure modules (`unicode`, `checks`, `fixname`) that take names and return findings — please keep it that way, and keep filesystem access confined to `walker` and `fixname::apply`.

## Ground rules

- Keep dependencies at zero. namefence is std-only by design — the Unicode tables are generated data, not a crate; adding any dependency needs a very strong justification in the PR description.
- No network calls ever, no telemetry. namefence reads names and (only with `fix --apply`) renames files the user pointed it at; nothing else touches the disk and nothing leaves the machine.
- Check IDs are stable: new checks append (NF014, NF015, ...), existing IDs are never renumbered or reused.
- Fix suggestions must stay collision-safe: any change to `fixname` must preserve the invariant that a planned name never case- or normalization-collides with a kept sibling or another planned name.
- Code comments and doc comments are written in English.

## Reporting bugs

Please include the `namefence --version` output, the finding (or missing finding) with `--format json`, and the exact byte sequence of the name involved — `printf 'name' | xxd` output is ideal, since many of these bugs are invisible in rendered text. For fix-planner bugs, the directory listing before and the plan output are what make the report reproducible.

## Security

If you find a security issue (e.g. a way to make `fix --apply` rename outside the target tree or overwrite an existing file), please do not open a public issue. Use GitHub's private vulnerability reporting on this repository instead.
