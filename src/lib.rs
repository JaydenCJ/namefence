//! namefence — lints filenames that will break on Windows, macOS, or cloud
//! sync.
//!
//! A name that is perfectly legal where it was created can be impossible to
//! check out, open, or sync somewhere else: Windows reserves device names
//! and eight punctuation characters, silently strips trailing dots and
//! spaces, and caps paths; macOS re-encodes names into decomposed Unicode;
//! case-insensitive volumes merge names that differ only by case; and cloud
//! clients reject or silently skip their own reserved names. namefence
//! detects all of it *before* the sync does — and, unlike generic
//! sanitizers, suggests a concrete, collision-safe rename for every
//! mechanical problem.
//!
//! The library crate exposes the pure layers (normalization, checks, fix
//! planning, reporting) for unit testing and reuse; the `namefence` binary
//! wires them into a CLI (see `src/main.rs`).

pub mod checks;
pub mod cli;
pub mod engine;
pub mod fixname;
pub mod report;
pub mod rules;
pub mod unicode;
pub mod unicode_data;
pub mod walker;

/// The crate version, single-sourced from Cargo.toml.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
