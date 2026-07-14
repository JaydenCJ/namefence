//! The check implementations: pure functions from a name (or a directory of
//! names) to findings.
//!
//! Everything here operates on plain byte slices and strings — no
//! filesystem access — so every rule is unit-testable with literals and the
//! same code serves the tree walker, the `stdin` mode and the fix planner.

use crate::rules::{find_check, Check};
use crate::unicode::{casefold_key, is_nfc, to_nfc, utf16_len};

/// A finding about one name, not yet attached to a path.
pub struct NameFinding {
    pub check: &'static Check,
    pub message: String,
    /// Suggested replacement for the whole name component, when a mechanical
    /// fix exists. `None` for advisory checks (NF011, NF012).
    pub fix: Option<String>,
}

impl NameFinding {
    fn new(id: &str, message: String, fix: Option<String>) -> NameFinding {
        NameFinding {
            check: find_check(id).expect("catalog id"),
            message,
            fix,
        }
    }
}

/// Render a name for humans: control and invisible code points become
/// `<U+XXXX>` so a finding about an invisible problem is actually visible.
pub fn display_name(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(s) => s
            .chars()
            .map(|c| {
                if c.is_control() || invisible_info(c).is_some() {
                    format!("<U+{:04X}>", c as u32)
                } else {
                    c.to_string()
                }
            })
            .collect(),
        Err(_) => bytes
            .iter()
            .map(|&b| {
                if b.is_ascii() && !b.is_ascii_control() {
                    (b as char).to_string()
                } else {
                    format!("<0x{b:02X}>")
                }
            })
            .collect(),
    }
}

// ---------------------------------------------------------------------------
// NF001 windows-reserved-name
// ---------------------------------------------------------------------------

/// The stem Windows compares against device names: everything before the
/// first dot, with trailing spaces and dots stripped the way Win32 does.
pub fn windows_stem(name: &str) -> &str {
    let stem = name.split('.').next().unwrap_or(name);
    stem.trim_end_matches([' ', '.'])
}

/// Is `stem` (already extracted) a reserved DOS device name?
pub fn is_reserved_stem(stem: &str) -> bool {
    let upper = stem.to_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            let mut chars = upper.chars();
            let prefix: String = chars.by_ref().take(3).collect();
            if prefix != "COM" && prefix != "LPT" {
                return false;
            }
            let rest: Vec<char> = chars.collect();
            rest.len() == 1 && matches!(rest[0], '0'..='9' | '\u{00B9}' | '\u{00B2}' | '\u{00B3}')
        }
    }
}

fn check_reserved_name(name: &str) -> Option<NameFinding> {
    let stem = windows_stem(name);
    if !is_reserved_stem(stem) {
        return None;
    }
    let fixed = format!("{}_{}", stem, &name[stem.len()..]);
    Some(NameFinding::new(
        "NF001",
        format!(
            "`{}` has the reserved DOS device stem `{}`; Windows cannot create, open or delete it",
            display_name(name.as_bytes()),
            stem.to_uppercase()
        ),
        Some(fixed),
    ))
}

// ---------------------------------------------------------------------------
// NF002 windows-illegal-char
// ---------------------------------------------------------------------------

/// The eight characters NTFS/Win32 reject inside a name component
/// (`/` is a separator on every platform and cannot occur here).
pub fn illegal_char_replacement(c: char) -> Option<&'static str> {
    match c {
        ':' | '|' | '\\' => Some("-"),
        '"' => Some("'"),
        '<' => Some("("),
        '>' => Some(")"),
        '?' | '*' => Some(""),
        _ => None,
    }
}

fn check_illegal_chars(name: &str) -> Option<NameFinding> {
    let mut found: Vec<char> = Vec::new();
    for c in name.chars() {
        if illegal_char_replacement(c).is_some() && !found.contains(&c) {
            found.push(c);
        }
    }
    if found.is_empty() {
        return None;
    }
    let list: Vec<String> = found.iter().map(|c| format!("`{c}`")).collect();
    let fixed: String = name
        .chars()
        .map(|c| illegal_char_replacement(c).map_or_else(|| c.to_string(), str::to_string))
        .collect();
    Some(NameFinding::new(
        "NF002",
        format!(
            "`{}` contains {} Windows-forbidden character(s): {}",
            display_name(name.as_bytes()),
            found.len(),
            list.join(", ")
        ),
        Some(fixed),
    ))
}

// ---------------------------------------------------------------------------
// NF003 control-character
// ---------------------------------------------------------------------------

fn check_control_chars(name: &str) -> Option<NameFinding> {
    let controls: Vec<char> = name.chars().filter(|c| c.is_ascii_control()).collect();
    if controls.is_empty() {
        return None;
    }
    let mut seen: Vec<String> = Vec::new();
    for c in &controls {
        let label = format!("U+{:04X}", *c as u32);
        if !seen.contains(&label) {
            seen.push(label);
        }
    }
    let fixed: String = name.chars().filter(|c| !c.is_ascii_control()).collect();
    Some(NameFinding::new(
        "NF003",
        format!(
            "`{}` contains {} control character(s): {}",
            display_name(name.as_bytes()),
            controls.len(),
            seen.join(", ")
        ),
        Some(fixed),
    ))
}

// ---------------------------------------------------------------------------
// NF004 trailing-dot-or-space / NF005 leading-space
// ---------------------------------------------------------------------------

fn check_trailing(name: &str) -> Option<NameFinding> {
    if !(name.ends_with('.') || name.ends_with(' ')) {
        return None;
    }
    let what = if name.ends_with('.') { "dot" } else { "space" };
    let trimmed = name.trim_end_matches([' ', '.']);
    let fixed = if trimmed.is_empty() { "_" } else { trimmed };
    Some(NameFinding::new(
        "NF004",
        format!(
            "`{}` ends with a {what}; Windows silently strips it, so the stored and visible names disagree",
            display_name(name.as_bytes())
        ),
        Some(fixed.to_string()),
    ))
}

fn check_leading_space(name: &str) -> Option<NameFinding> {
    if !name.starts_with(' ') {
        return None;
    }
    Some(NameFinding::new(
        "NF005",
        format!(
            "`{}` begins with a space; Explorer and OneDrive reject or hide such names",
            display_name(name.as_bytes())
        ),
        Some(name.trim_start_matches(' ').to_string()),
    ))
}

// ---------------------------------------------------------------------------
// NF008 non-nfc
// ---------------------------------------------------------------------------

fn check_non_nfc(name: &str) -> Option<NameFinding> {
    if is_nfc(name) {
        return None;
    }
    let nfc = to_nfc(name);
    Some(NameFinding::new(
        "NF008",
        format!(
            "`{}` is not NFC-normalized ({} code points; the NFC form has {}); \
byte-comparing sync tools treat the two encodings as different files",
            display_name(name.as_bytes()),
            name.chars().count(),
            nfc.chars().count()
        ),
        Some(nfc),
    ))
}

// ---------------------------------------------------------------------------
// NF009 invisible-character
// ---------------------------------------------------------------------------

/// What to do with an invisible or lookalike code point in a fix.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InvisibleAction {
    Remove,
    ReplaceWithSpace,
}

/// Classify a code point as invisible/lookalike, with a human label.
pub fn invisible_info(c: char) -> Option<(&'static str, InvisibleAction)> {
    use InvisibleAction::{Remove, ReplaceWithSpace};
    match c {
        '\u{00A0}' | '\u{2000}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{1680}' => {
            Some(("lookalike space", ReplaceWithSpace))
        }
        '\u{200B}' | '\u{2060}' | '\u{FEFF}' | '\u{180E}' => Some(("zero-width space", Remove)),
        '\u{200C}' | '\u{200D}' => Some(("zero-width joiner", Remove)),
        '\u{00AD}' => Some(("soft hyphen", Remove)),
        '\u{200E}'
        | '\u{200F}'
        | '\u{061C}'
        | '\u{202A}'..='\u{202E}'
        | '\u{2066}'..='\u{2069}' => Some(("bidi control", Remove)),
        _ => None,
    }
}

fn check_invisible(name: &str) -> Option<NameFinding> {
    let mut labels: Vec<String> = Vec::new();
    let mut count = 0usize;
    for c in name.chars() {
        if let Some((label, _)) = invisible_info(c) {
            count += 1;
            let entry = format!("{label} U+{:04X}", c as u32);
            if !labels.contains(&entry) {
                labels.push(entry);
            }
        }
    }
    if count == 0 {
        return None;
    }
    let fixed: String = name
        .chars()
        .filter_map(|c| match invisible_info(c) {
            Some((_, InvisibleAction::Remove)) => None,
            Some((_, InvisibleAction::ReplaceWithSpace)) => Some(' '),
            None => Some(c),
        })
        .collect();
    Some(NameFinding::new(
        "NF009",
        format!(
            "`{}` contains {count} invisible or lookalike character(s): {}",
            display_name(name.as_bytes()),
            labels.join(", ")
        ),
        Some(fixed),
    ))
}

// ---------------------------------------------------------------------------
// NF010 component-too-long
// ---------------------------------------------------------------------------

/// Both budgets a single component must satisfy: 255 UTF-8 bytes (ext4 and
/// friends) and 255 UTF-16 code units (NTFS, and macOS at the API layer).
pub const COMPONENT_LIMIT: usize = 255;

/// Split a name into (stem, extension-with-dot). Dotfiles have no extension;
/// extensions longer than 16 characters are treated as part of the stem.
pub fn split_extension(name: &str) -> (&str, &str) {
    match name.rfind('.') {
        Some(idx) if idx > 0 && name.len() - idx <= 16 => name.split_at(idx),
        _ => (name, ""),
    }
}

/// Truncate `name` so it fits both component budgets, preserving the
/// extension and never splitting a character.
pub fn truncate_component(name: &str) -> String {
    let (stem, ext) = split_extension(name);
    let mut kept: String = stem.to_string();
    loop {
        let candidate_bytes = kept.len() + ext.len();
        let candidate_utf16 = utf16_len(&kept) + utf16_len(ext);
        if candidate_bytes <= COMPONENT_LIMIT && candidate_utf16 <= COMPONENT_LIMIT {
            break;
        }
        match kept.char_indices().next_back() {
            Some((idx, _)) => kept.truncate(idx),
            None => break,
        }
    }
    let kept = kept.trim_end_matches([' ', '.']);
    if kept.is_empty() && ext.is_empty() {
        return "_".to_string();
    }
    format!("{kept}{ext}")
}

fn check_component_length(name: &str) -> Option<NameFinding> {
    let bytes = name.len();
    let units = utf16_len(name);
    if bytes <= COMPONENT_LIMIT && units <= COMPONENT_LIMIT {
        return None;
    }
    Some(NameFinding::new(
        "NF010",
        format!(
            "name is {bytes} UTF-8 bytes / {units} UTF-16 units; the portable limit is \
{COMPONENT_LIMIT} of each"
        ),
        Some(truncate_component(name)),
    ))
}

// ---------------------------------------------------------------------------
// NF011 path-too-long (needs the whole relative path, called by the engine)
// ---------------------------------------------------------------------------

pub fn check_path_length(rel_path: &str, max_path: usize) -> Option<NameFinding> {
    let units = utf16_len(rel_path);
    if units <= max_path {
        return None;
    }
    Some(NameFinding::new(
        "NF011",
        format!(
            "relative path is {units} UTF-16 units, over the --max-path budget of {max_path}; \
Win32 MAX_PATH counts 260 for the absolute path"
        ),
        None,
    ))
}

// ---------------------------------------------------------------------------
// NF012 cloud-reserved-name
// ---------------------------------------------------------------------------

fn cloud_reserved_reason(name: &str) -> Option<&'static str> {
    let lower = name.to_lowercase();
    match lower.as_str() {
        "desktop.ini" => Some("rejected by OneDrive and silently skipped by Dropbox"),
        "thumbs.db" => Some("silently skipped by Dropbox"),
        ".ds_store" | "icon\r" => Some("macOS metadata; silently skipped by Dropbox"),
        ".dropbox" | ".dropbox.attr" => Some("reserved by the Dropbox client itself"),
        _ => {
            if lower.starts_with("~$") {
                Some("Office lock-file prefix; skipped by OneDrive and Dropbox")
            } else if lower.contains("_vti_") {
                Some("rejected by OneDrive/SharePoint")
            } else {
                None
            }
        }
    }
}

fn check_cloud_reserved(name: &str) -> Option<NameFinding> {
    let reason = cloud_reserved_reason(name)?;
    Some(NameFinding::new(
        "NF012",
        format!(
            "`{}` is {reason}; if it holds real data it will quietly not sync",
            display_name(name.as_bytes())
        ),
        None,
    ))
}

// ---------------------------------------------------------------------------
// NF013 invalid-utf8
// ---------------------------------------------------------------------------

fn check_invalid_utf8(bytes: &[u8]) -> Option<NameFinding> {
    let err = std::str::from_utf8(bytes).err()?;
    let offset = err.valid_up_to();
    // Lossy decoding turns each invalid sequence into U+FFFD; substituting
    // `_` yields a plain, everywhere-legal suggestion.
    let lossy: String = String::from_utf8_lossy(bytes).replace('\u{FFFD}', "_");
    Some(NameFinding::new(
        "NF013",
        format!(
            "name is not valid UTF-8 (first invalid byte 0x{:02X} at offset {offset}); \
macOS, Windows and every sync client require Unicode names",
            bytes[offset]
        ),
        Some(lossy),
    ))
}

// ---------------------------------------------------------------------------
// Per-name driver
// ---------------------------------------------------------------------------

/// Run every per-name check against one component. Order = catalog order.
/// `.` and `..` are path syntax, not names, and are never flagged.
pub fn check_component(bytes: &[u8]) -> Vec<NameFinding> {
    if bytes.is_empty() || bytes == b"." || bytes == b".." {
        return Vec::new();
    }
    let name = match std::str::from_utf8(bytes) {
        Ok(s) => s,
        Err(_) => return check_invalid_utf8(bytes).into_iter().collect(),
    };
    [
        check_reserved_name(name),
        check_illegal_chars(name),
        check_control_chars(name),
        check_trailing(name),
        check_leading_space(name),
        check_non_nfc(name),
        check_invisible(name),
        check_component_length(name),
        check_cloud_reserved(name),
    ]
    .into_iter()
    .flatten()
    .collect()
}

// ---------------------------------------------------------------------------
// Per-directory collision checks (NF006, NF007)
// ---------------------------------------------------------------------------

/// A collision finding: the name at `index` collides with the earlier name
/// at `first_index` under the given check.
pub struct CollisionFinding {
    pub index: usize,
    pub first_index: usize,
    pub finding: NameFinding,
}

/// Detect case collisions (NF006) and normalization collisions (NF007) among
/// sibling names. Names must be in a deterministic order; the first name of
/// each colliding group is kept, later ones are reported. A pair that is
/// NFC-equal reports NF007 only (the more precise diagnosis); NF006 covers
/// pairs that need case folding to collide.
pub fn check_siblings(names: &[&[u8]]) -> Vec<CollisionFinding> {
    let mut nfc_first: Vec<(String, usize)> = Vec::new();
    let mut fold_first: Vec<(String, usize)> = Vec::new();
    let mut out = Vec::new();

    for (i, raw) in names.iter().enumerate() {
        let Ok(name) = std::str::from_utf8(raw) else {
            continue; // NF013 already covers it; no stable key exists
        };
        let nfc = to_nfc(name);
        let fold = casefold_key(name);

        let nfc_hit = nfc_first.iter().find(|(k, _)| *k == nfc).map(|&(_, i)| i);
        let fold_hit = fold_first.iter().find(|(k, _)| *k == fold).map(|&(_, i)| i);

        if let Some(fi) = nfc_hit {
            let other = String::from_utf8_lossy(names[fi]).into_owned();
            out.push(CollisionFinding {
                index: i,
                first_index: fi,
                finding: NameFinding::new(
                    "NF007",
                    format!(
                        "`{}` ({} code points) and sibling `{}` ({} code points) are the same \
text in different Unicode encodings; macOS and sync tools see one file",
                        display_name(raw),
                        name.chars().count(),
                        display_name(names[fi]),
                        other.chars().count()
                    ),
                    None, // the fix planner numbers the survivor deterministically
                ),
            });
        } else if let Some(fi) = fold_hit {
            out.push(CollisionFinding {
                index: i,
                first_index: fi,
                finding: NameFinding::new(
                    "NF006",
                    format!(
                        "`{}` collides with sibling `{}` on case-insensitive filesystems \
(Windows, macOS default)",
                        display_name(raw),
                        display_name(names[fi])
                    ),
                    None,
                ),
            });
        } else {
            nfc_first.push((nfc, i));
            fold_first.push((fold, i));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(bytes: &[u8]) -> Vec<&'static str> {
        check_component(bytes).iter().map(|f| f.check.id).collect()
    }

    fn fix_for(bytes: &[u8], id: &str) -> Option<String> {
        check_component(bytes)
            .into_iter()
            .find(|f| f.check.id == id)
            .and_then(|f| f.fix)
    }

    #[test]
    fn clean_names_produce_nothing() {
        for name in [
            "report_2024.txt",
            "src",
            ".gitignore",
            "caf\u{00E9}.md",
            "写真.jpg",
        ] {
            assert!(ids(name.as_bytes()).is_empty(), "{name} flagged");
        }
    }

    #[test]
    fn dot_and_dotdot_are_never_flagged() {
        assert!(ids(b".").is_empty());
        assert!(ids(b"..").is_empty());
        assert!(ids(b"").is_empty());
    }

    #[test]
    fn reserved_names_with_and_without_extensions() {
        assert_eq!(ids(b"CON"), ["NF001"]);
        assert_eq!(ids(b"aux.tar.gz"), ["NF001"]);
        assert_eq!(ids(b"NuL.txt"), ["NF001"]);
        assert_eq!(ids(b"lpt9"), ["NF001"]);
        assert_eq!(ids("com\u{00B2}.log".as_bytes()), ["NF001"]);
        assert_eq!(fix_for(b"aux.tar.gz", "NF001").unwrap(), "aux_.tar.gz");
        assert_eq!(fix_for(b"CON", "NF001").unwrap(), "CON_");
    }

    #[test]
    fn reserved_near_misses_pass() {
        // COM10 exists happily; CONS, AUXILIARY and console.log are fine.
        for name in [
            "COM10",
            "LPT10",
            "CONS",
            "auxiliary.txt",
            "console.log",
            "communal",
        ] {
            assert!(ids(name.as_bytes()).is_empty(), "{name} wrongly flagged");
        }
    }

    #[test]
    fn reserved_stem_survives_win32_trailing_strip() {
        // "aux ." -> Win32 strips to "aux": reserved AND trailing-dot.
        let found = ids(b"aux .");
        assert!(found.contains(&"NF001"), "{found:?}");
        assert!(found.contains(&"NF004"), "{found:?}");
    }

    #[test]
    fn illegal_chars_are_listed_once_each_and_mapped() {
        let f = check_component(b"a:b:c*d.txt");
        let hit = f.iter().find(|f| f.check.id == "NF002").unwrap();
        assert!(hit.message.contains("2 Windows-forbidden"));
        assert!(hit.message.contains("`:`") && hit.message.contains("`*`"));
        assert_eq!(hit.fix.as_deref(), Some("a-b-cd.txt"));
        assert_eq!(
            fix_for(b"say \"hi\"<now>?.md", "NF002").unwrap(),
            "say 'hi'(now).md"
        );
    }

    #[test]
    fn control_chars_flagged_with_codepoints() {
        let f = check_component(b"line\nbreak\t.log");
        let hit = f.iter().find(|f| f.check.id == "NF003").unwrap();
        assert!(hit.message.contains("U+000A") && hit.message.contains("U+0009"));
        assert_eq!(hit.fix.as_deref(), Some("linebreak.log"));
    }

    #[test]
    fn icon_cr_is_both_control_and_cloud_reserved() {
        let found = ids(b"Icon\r");
        assert!(found.contains(&"NF003"), "{found:?}");
        assert!(found.contains(&"NF012"), "{found:?}");
    }

    #[test]
    fn trailing_dot_and_space_trim_to_something() {
        assert_eq!(fix_for(b"report.", "NF004").unwrap(), "report");
        assert_eq!(fix_for(b"notes ", "NF004").unwrap(), "notes");
        assert_eq!(fix_for(b"...", "NF004").unwrap(), "_"); // nothing left
        assert!(ids(b"normal.txt").is_empty()); // inner dots are fine
    }

    #[test]
    fn leading_space_trimmed() {
        assert_eq!(fix_for(b"  draft.md", "NF005").unwrap(), "draft.md");
        assert!(ids(b"mid space.md").is_empty());
    }

    #[test]
    fn nfd_name_flagged_with_nfc_fix() {
        let nfd = "cafe\u{0301}.txt";
        let f = check_component(nfd.as_bytes());
        let hit = f.iter().find(|f| f.check.id == "NF008").unwrap();
        assert_eq!(hit.fix.as_deref(), Some("caf\u{00E9}.txt"));
        assert!(hit.message.contains("9 code points"));
        assert!(ids("caf\u{00E9}.txt".as_bytes()).is_empty());
    }

    #[test]
    fn invisible_characters_classified_and_fixed() {
        let name = "fee\u{200B}\u{00A0}plan\u{202E}.txt";
        let f = check_component(name.as_bytes());
        let hit = f.iter().find(|f| f.check.id == "NF009").unwrap();
        assert!(hit.message.contains("zero-width space U+200B"));
        assert!(hit.message.contains("lookalike space U+00A0"));
        assert!(hit.message.contains("bidi control U+202E"));
        assert_eq!(hit.fix.as_deref(), Some("fee plan.txt"));
    }

    #[test]
    fn component_length_checks_both_units() {
        let ascii_long = "a".repeat(256);
        assert_eq!(ids(ascii_long.as_bytes()), ["NF010"]);
        // 90 CJK chars: 270 UTF-8 bytes but only 90 UTF-16 units -> still flagged
        // (byte budget), and the fix must not split a character.
        let cjk = "\u{5199}".repeat(90);
        let f = check_component(cjk.as_bytes());
        assert_eq!(f[0].check.id, "NF010");
        let fixed = f[0].fix.as_deref().unwrap();
        assert!(fixed.len() <= 255 && fixed.chars().count() == 85);
        // 200 emoji: 800 bytes, 400 UTF-16 units -> flagged on both budgets.
        let emoji = "\u{1F600}".repeat(200);
        assert_eq!(ids(emoji.as_bytes()), ["NF010"]);
        let ok = "a".repeat(255);
        assert!(ids(ok.as_bytes()).is_empty());
    }

    #[test]
    fn truncation_preserves_extension() {
        let name = format!("{}.tar.gz", "x".repeat(300));
        let fixed = truncate_component(&name);
        assert!(fixed.ends_with(".gz"));
        assert!(fixed.len() <= 255);
        // A dotfile with no real extension truncates as a whole.
        let dotfile = format!(".{}", "y".repeat(300));
        let fixed2 = truncate_component(&dotfile);
        assert_eq!(fixed2.len(), 255);
        assert!(fixed2.starts_with('.'));
    }

    #[test]
    fn split_extension_edge_cases() {
        assert_eq!(split_extension("a.txt"), ("a", ".txt"));
        assert_eq!(split_extension(".bashrc"), (".bashrc", ""));
        assert_eq!(split_extension("noext"), ("noext", ""));
        assert_eq!(
            split_extension("weird.aaaaaaaaaaaaaaaaaaaaaa"),
            ("weird.aaaaaaaaaaaaaaaaaaaaaa", "")
        );
    }

    #[test]
    fn path_length_budget() {
        assert!(check_path_length("short/path.txt", 240).is_none());
        let long = format!("{}/file.txt", "d".repeat(300));
        let f = check_path_length(&long, 240).unwrap();
        assert_eq!(f.check.id, "NF011");
        assert!(f.fix.is_none());
        assert!(check_path_length(&long, 500).is_none());
    }

    #[test]
    fn cloud_reserved_names_by_platform() {
        assert_eq!(ids(b"Thumbs.db"), ["NF012"]);
        assert_eq!(ids(b"desktop.ini"), ["NF012"]);
        assert_eq!(ids(b".DS_Store"), ["NF012"]);
        assert_eq!(ids(b"~$budget.xlsx"), ["NF012"]);
        assert_eq!(ids(b"my_vti_dir"), ["NF012"]);
        assert!(ids(b"desktop.txt").is_empty());
        assert!(ids(b"thumbsup.db").is_empty());
    }

    #[test]
    fn invalid_utf8_reports_offset_and_suggests_ascii() {
        let f = check_component(b"caf\xE9.txt"); // Latin-1 e-acute
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].check.id, "NF013");
        assert!(f[0].message.contains("0xE9 at offset 3"));
        assert_eq!(f[0].fix.as_deref(), Some("caf_.txt"));
    }

    #[test]
    fn one_name_can_carry_many_findings() {
        let found = ids(b"aux. draft: ");
        assert_eq!(found, ["NF001", "NF002", "NF004"]);
    }

    #[test]
    fn reserved_stem_with_inner_trailing_space() {
        // "nul .txt": the stem before the first dot is "nul " and Win32
        // trims it to the NUL device.
        let f = check_component(b"nul .txt");
        let hit = f.iter().find(|f| f.check.id == "NF001").unwrap();
        assert_eq!(hit.fix.as_deref(), Some("nul_ .txt"));
    }

    #[test]
    fn case_collision_detected_and_attributed() {
        let names: Vec<&[u8]> = vec![b"README.md", b"readme.md", b"other.txt"];
        let hits = check_siblings(&names);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].index, 1);
        assert_eq!(hits[0].first_index, 0);
        assert_eq!(hits[0].finding.check.id, "NF006");
        assert!(hits[0].finding.message.contains("`README.md`"));
    }

    #[test]
    fn unicode_case_collision_across_normalization_forms() {
        // NFC uppercase vs NFD lowercase: needs normalization AND folding.
        let a = "R\u{00C9}SUM\u{00C9}.doc";
        let b = "re\u{0301}sume\u{0301}.doc";
        let names: Vec<&[u8]> = vec![a.as_bytes(), b.as_bytes()];
        let hits = check_siblings(&names);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].finding.check.id, "NF006");
    }

    #[test]
    fn normalization_collision_beats_case_collision() {
        // Byte-different but NFC-equal: precise diagnosis is NF007, not NF006.
        let a = "caf\u{00E9}.txt";
        let b = "cafe\u{0301}.txt";
        let names: Vec<&[u8]> = vec![a.as_bytes(), b.as_bytes()];
        let hits = check_siblings(&names);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].finding.check.id, "NF007");
        // The NFD name has 9 code points, its NFC sibling 8.
        assert!(hits[0].finding.message.contains("9 code points"));
        assert!(hits[0].finding.message.contains("8 code points"));
    }

    #[test]
    fn three_way_collision_reports_two_findings_against_the_first() {
        let names: Vec<&[u8]> = vec![b"a.txt", b"A.txt", b"A.TXT"];
        let hits = check_siblings(&names);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|h| h.first_index == 0));
    }

    #[test]
    fn distinct_names_do_not_collide() {
        let names: Vec<&[u8]> = vec![b"a.txt", b"b.txt", "stra\u{00DF}e".as_bytes(), b"strasse"];
        // ß does not case-fold to ss in filesystem comparisons; no collision.
        assert!(check_siblings(&names).is_empty());
    }

    #[test]
    fn display_name_makes_invisibles_visible() {
        assert_eq!(display_name(b"a\nb"), "a<U+000A>b");
        assert_eq!(display_name("a\u{200B}b".as_bytes()), "a<U+200B>b");
        assert_eq!(display_name(b"caf\xE9"), "caf<0xE9>");
        assert_eq!(display_name(b"plain.txt"), "plain.txt");
    }
}
