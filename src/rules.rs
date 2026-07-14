//! The check catalog: stable IDs, severities, target platforms, one-line
//! descriptions and the long-form `explain` texts.
//!
//! Every finding namefence can produce is declared here so that `--only`,
//! `--skip`, `--targets`, `namefence checks` and `namefence explain` all
//! operate on one source of truth. IDs are stable across releases; new
//! checks append, they never renumber.

use std::fmt;

/// How bad a finding is. Ordering matters: `Error > Warning > Info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info => "info",
        }
    }
}

impl fmt::Display for Severity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Where a name breaks. A check runs when its target set intersects the
/// `--targets` selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Target {
    Windows,
    Macos,
    Linux,
    Cloud,
}

impl Target {
    pub const ALL: [Target; 4] = [Target::Windows, Target::Macos, Target::Linux, Target::Cloud];

    pub fn as_str(self) -> &'static str {
        match self {
            Target::Windows => "windows",
            Target::Macos => "macos",
            Target::Linux => "linux",
            Target::Cloud => "cloud",
        }
    }

    pub fn parse(s: &str) -> Option<Target> {
        match s {
            "windows" => Some(Target::Windows),
            "macos" => Some(Target::Macos),
            "linux" => Some(Target::Linux),
            "cloud" => Some(Target::Cloud),
            _ => None,
        }
    }
}

/// One entry in the catalog.
pub struct Check {
    pub id: &'static str,
    pub name: &'static str,
    pub severity: Severity,
    pub targets: &'static [Target],
    /// One-line description for `namefence checks`.
    pub summary: &'static str,
    /// Long-form story for `namefence explain`.
    pub explain: &'static str,
}

use Target::{Cloud, Linux, Macos, Windows};

/// The full catalog, in ID order.
pub const CHECKS: &[Check] = &[
    Check {
        id: "NF001",
        name: "windows-reserved-name",
        severity: Severity::Error,
        targets: &[Windows, Cloud],
        summary: "name (or its stem) is a reserved DOS device name: CON, PRN, AUX, NUL, COM0-9, LPT0-9",
        explain: "Windows reserves the DOS device names CON, PRN, AUX, NUL, COM0..COM9, \
LPT0..LPT9 (and the superscript variants COM\u{00B9}\u{00B2}\u{00B3}, LPT\u{00B9}\u{00B2}\u{00B3}) \
in every directory, in any case, and *with any extension*: `aux.tar.gz` is just as \
unusable as `AUX`. Such a file created on Linux cannot be checked out, opened or \
deleted through normal Windows APIs, and OneDrive refuses to sync it. The stem is \
compared case-insensitively up to the first dot, after stripping trailing spaces \
and dots the way the Win32 layer does.\n\nfix: namefence suggests appending `_` to \
the reserved stem (`aux.tar.gz` -> `aux_.tar.gz`), which keeps the name readable \
and sortable.",
    },
    Check {
        id: "NF002",
        name: "windows-illegal-char",
        severity: Severity::Error,
        targets: &[Windows, Cloud],
        summary: "name contains a character Windows forbids: < > : \" | ? * or backslash",
        explain: "NTFS and the Win32 API reject nine characters that are perfectly legal \
on Linux and macOS: `<` `>` `:` `\"` `|` `?` `*` `\\` and `/` (the last cannot occur \
inside a component on any platform, so namefence checks the other eight). A `git \
clone` of a repository containing `results:final.csv` fails on Windows with a \
checkout error, and every major cloud client rejects the upload. Note that macOS \
Finder displays `:` as `/` and vice versa \u{2014} a name legal on one desktop can be \
unrepresentable on another.\n\nfix: namefence maps each character to a safe, \
visually close substitute: `:` and `|` and `\\` become `-`, `\"` becomes `'`, `<` \
and `>` become `(` and `)`, `?` and `*` are dropped.",
    },
    Check {
        id: "NF003",
        name: "control-character",
        severity: Severity::Error,
        targets: &[Windows, Cloud],
        summary: "name contains an ASCII control character (0x00-0x1F, 0x7F)",
        explain: "Linux allows every byte except `/` and NUL in a filename, so names \
containing newlines, tabs or escape characters do occur \u{2014} usually created by a \
buggy script. Windows rejects code points below 0x20 outright, cloud clients \
refuse them, terminals mangle them, and a newline inside a filename breaks every \
line-oriented tool downstream (`xargs`, `while read`, log parsers). The classic \
macOS offender `Icon\\r` (Finder custom-icon marker) is also caught by this \
check.\n\nfix: namefence suggests removing the control characters.",
    },
    Check {
        id: "NF004",
        name: "trailing-dot-or-space",
        severity: Severity::Error,
        targets: &[Windows, Cloud],
        summary: "name ends with a dot or a space, which Windows silently strips",
        explain: "The Win32 layer strips trailing dots and spaces before touching the \
filesystem: `report.` and `report` are the same file to Windows but different \
files to Linux. Sync a directory containing both and one of them silently \
overwrites the other; a name that is *only* reachable with its trailing dot \
cannot be opened or deleted from Explorer at all. OneDrive and Dropbox both \
reject such names at upload time.\n\nfix: namefence suggests trimming the \
trailing dots and spaces (falling back to `_` if nothing remains).",
    },
    Check {
        id: "NF005",
        name: "leading-space",
        severity: Severity::Warning,
        targets: &[Windows, Cloud],
        summary: "name begins with a space",
        explain: "Leading spaces are technically storable on NTFS but Explorer, the \
file dialogs and most shells make the file look broken or unreachable, and \
OneDrive rejects names that begin with a space. They also sort the file \
invisibly to the top of every listing, which is how they usually go unnoticed \
until a sync fails.\n\nfix: namefence suggests trimming the leading spaces.",
    },
    Check {
        id: "NF006",
        name: "case-collision",
        severity: Severity::Error,
        targets: &[Windows, Macos, Cloud],
        summary: "two names in one directory differ only by letter case",
        explain: "`README.md` and `readme.md` are two files on ext4 but one file on \
the case-insensitive filesystems that Windows and macOS ship by default. A git \
checkout of both produces one file with the other's content and a permanently \
dirty working tree; Dropbox and OneDrive create conflict copies or drop one \
side. namefence compares names after canonical normalization and full Unicode \
case folding, so `R\u{00C9}SUM\u{00C9}.doc` vs `r\u{00E9}sum\u{00E9}.doc` is caught \
too, matching how NTFS and APFS actually compare.\n\nfix: namefence keeps the \
first name and suggests numbered renames (`readme-2.md`) for the rest.",
    },
    Check {
        id: "NF007",
        name: "normalization-collision",
        severity: Severity::Error,
        targets: &[Macos, Cloud],
        summary: "two names in one directory are the same text in different Unicode encodings",
        explain: "A composed `caf\u{00E9}` (5 code points) and a decomposed \
`cafe\u{0301}` (6 code points) are distinct names on ext4 but the *same* name on \
macOS, which normalizes filenames. Directories accumulate such pairs after a \
round trip through a Mac, and they are the signature failure behind endless \
Syncthing and Dropbox forum threads: the two sides ping-pong the \"duplicate\", \
or one silently overwrites the other on the next full sync. namefence compares \
canonical (NFC) forms to find these pairs before the sync does.\n\nfix: \
namefence keeps the first name and suggests a numbered rename for the other; \
usually one of the pair is a stale duplicate you can simply delete.",
    },
    Check {
        id: "NF008",
        name: "non-nfc",
        severity: Severity::Warning,
        targets: &[Macos, Cloud],
        summary: "name is not in NFC form (decomposed accents, the macOS round-trip artifact)",
        explain: "Linux and Windows store filenames as the byte sequence you give \
them; macOS historically re-encodes them into decomposed (NFD-style) form. A \
file that has round-tripped through a Mac comes back byte-different while \
looking identical on screen. Byte-comparing sync tools then re-upload it \
forever, dedup tools see two files, tab completion stops matching what you \
type, and `git status` shows phantom changes. Keeping every name in NFC \u{2014} \
the form Linux keyboards and Windows produce natively \u{2014} makes the problem \
impossible.\n\nfix: namefence suggests the NFC re-encoding of the same visible \
name (the rename is invisible on screen but changes the bytes).",
    },
    Check {
        id: "NF009",
        name: "invisible-character",
        severity: Severity::Warning,
        targets: &[Windows, Macos, Linux, Cloud],
        summary: "name contains zero-width, bidi-control or lookalike-space characters",
        explain: "Zero-width spaces and joiners, soft hyphens, BOMs, bidi controls \
and no-break spaces render as nothing or as an ordinary space, producing two \
names that look identical in every file manager yet never match each other. \
They arrive through copy-paste from chat tools and PDFs, and bidi controls in \
particular can make a name *display* in a different order than it compares. \
This check is severity `warning` because the names work everywhere \u{2014} they \
just deceive humans and diff tools.\n\nfix: namefence suggests deleting \
zero-width and control code points and replacing lookalike spaces with plain \
spaces.",
    },
    Check {
        id: "NF010",
        name: "component-too-long",
        severity: Severity::Error,
        targets: &[Windows, Macos, Linux, Cloud],
        summary: "single name longer than 255 UTF-8 bytes or 255 UTF-16 units",
        explain: "Every mainstream filesystem caps one name component at 255 units \u{2014} \
but the unit differs: bytes on ext4, UTF-16 code units on NTFS, and macOS \
enforces 255 UTF-16 units at the API layer. A 90-character CJK name fits \
easily as UTF-16 yet occupies 270 UTF-8 bytes and cannot be created on Linux; \
a long emoji name does the reverse. namefence flags a component when either \
budget is exceeded, so the name works everywhere.\n\nfix: namefence suggests \
truncating the stem at a character boundary while preserving the extension.",
    },
    Check {
        id: "NF011",
        name: "path-too-long",
        severity: Severity::Warning,
        targets: &[Windows, Cloud],
        summary: "full relative path exceeds the --max-path budget (default 240)",
        explain: "Classic Win32 APIs stop at MAX_PATH = 260 characters for the \
*absolute* path, and unpatched Explorer, installers and countless tools still \
enforce it; OneDrive additionally caps the full path at 400 characters. Because \
the absolute prefix (`C:\\Users\\...\\Dropbox\\`) eats budget you cannot see from \
inside the tree, namefence measures the path relative to the scan root against \
a conservative default of 240 (override with `--max-path`). Deep node_modules- \
style trees and \"copy of copy of\" folders are the usual offenders.\n\nfix: no \
mechanical rename can fix depth; shorten directory names near the root.",
    },
    Check {
        id: "NF012",
        name: "cloud-reserved-name",
        severity: Severity::Warning,
        targets: &[Cloud],
        summary: "name that cloud sync clients refuse or silently skip (desktop.ini, ~$..., .ds_store, ...)",
        explain: "Cloud clients hard-code name lists they will not sync: OneDrive \
rejects `desktop.ini` and any name containing `_vti_`; Dropbox silently skips \
`desktop.ini`, `thumbs.db`, `.ds_store`, `icon\\r`, `.dropbox` and \
`.dropbox.attr`; both skip Office lock files starting with `~$`. Silent skips \
are the dangerous half: the file simply never appears on the other machine and \
nothing tells you. If real data hides in such a name (a tool that dumps state \
into `thumbs.db`, say), it will quietly not be backed up.\n\nfix: none \
suggested automatically \u{2014} these are usually system litter to delete or \
exclude; rename by hand if the file carries real data.",
    },
    Check {
        id: "NF013",
        name: "invalid-utf8",
        severity: Severity::Error,
        targets: &[Windows, Macos, Cloud],
        summary: "name is not valid UTF-8",
        explain: "Linux filenames are raw bytes, so a name written under a legacy \
locale (Latin-1, Shift-JIS) or corrupted by a buggy tool can contain byte \
sequences that decode as nothing. macOS and Windows both require their \
filenames to be valid Unicode, so these names cannot be represented on either, \
and every sync client errors out or skips them. They also render as `?` \
mojibake in terminals and break any UTF-8-assuming script.\n\nfix: namefence \
suggests the name with each invalid byte replaced by `_`; recover the intended \
name by converting from the original encoding if you know it.",
    },
];

/// Look up a check by ID (`NF004`) or name (`trailing-dot-or-space`),
/// case-insensitively for the ID part.
pub fn find_check(key: &str) -> Option<&'static Check> {
    let upper = key.to_ascii_uppercase();
    CHECKS
        .iter()
        .find(|c| c.id == upper || c.name.eq_ignore_ascii_case(key))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_sequential_and_unique() {
        for (i, c) in CHECKS.iter().enumerate() {
            assert_eq!(c.id, format!("NF{:03}", i + 1));
        }
    }

    #[test]
    fn every_check_has_targets_and_texts() {
        for c in CHECKS {
            assert!(!c.targets.is_empty(), "{} has no targets", c.id);
            assert!(!c.summary.is_empty());
            assert!(c.explain.len() > 100, "{} explain too thin", c.id);
            assert!(
                c.explain.contains("fix:"),
                "{} explain lacks a fix note",
                c.id
            );
        }
    }

    #[test]
    fn lookup_by_id_name_and_case() {
        assert_eq!(find_check("NF001").unwrap().name, "windows-reserved-name");
        assert_eq!(find_check("nf001").unwrap().name, "windows-reserved-name");
        assert_eq!(find_check("case-collision").unwrap().id, "NF006");
        assert_eq!(find_check("Case-Collision").unwrap().id, "NF006");
        assert!(find_check("NF099").is_none());
        assert!(find_check("").is_none());
    }

    #[test]
    fn severity_ordering_drives_fail_on() {
        assert!(Severity::Error > Severity::Warning);
        assert!(Severity::Warning > Severity::Info);
    }

    #[test]
    fn target_parse_round_trips() {
        for t in Target::ALL {
            assert_eq!(Target::parse(t.as_str()), Some(t));
        }
        assert_eq!(Target::parse("solaris"), None);
    }
}
