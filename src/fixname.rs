//! The fix engine: sanitize one name, and plan collision-free renames for a
//! whole tree.
//!
//! `sanitize_name` merges every mechanical fix into one final name (the
//! per-finding fixes shown by `check` address one problem each; a rename
//! should address all of them at once). `plan` then makes the results safe
//! to apply: no suggested name may collide — case-insensitively and
//! normalization-insensitively — with a kept name or another suggestion in
//! the same directory. That last step is what generic sanitizers skip, and
//! it is exactly how a "fixed" tree ends up overwriting files on Windows.

use std::io;
use std::path::{Path, PathBuf};

use crate::checks::{
    check_component, illegal_char_replacement, invisible_info, is_reserved_stem, split_extension,
    truncate_component, windows_stem, InvisibleAction, COMPONENT_LIMIT,
};
use crate::engine::Selection;
use crate::rules::find_check;
use crate::unicode::{casefold_key, to_nfc, utf16_len};
use crate::walker::{walk, Entry};

/// Is a given fix stage enabled? Each stage belongs to the check that
/// motivates it, so `--only`/`--skip`/`--targets` shape fixes too.
fn on(sel: &Selection, id: &str) -> bool {
    sel.is_enabled(find_check(id).expect("catalog id"))
}

/// Merge every enabled mechanical fix into one final name. Idempotent:
/// `sanitize_name(sanitize_name(n)) == sanitize_name(n)`.
pub fn sanitize_name(bytes: &[u8], sel: &Selection) -> String {
    // NF013: non-UTF-8 bytes become `_` first; later stages need a str.
    let mut name: String = match std::str::from_utf8(bytes) {
        Ok(s) => s.to_string(),
        Err(_) if on(sel, "NF013") => String::from_utf8_lossy(bytes).replace('\u{FFFD}', "_"),
        Err(_) => return String::from_utf8_lossy(bytes).into_owned(),
    };
    if name == "." || name == ".." {
        return name;
    }
    if on(sel, "NF003") {
        name.retain(|c| !c.is_ascii_control());
    }
    if on(sel, "NF002") {
        name = name
            .chars()
            .map(|c| illegal_char_replacement(c).map_or_else(|| c.to_string(), str::to_string))
            .collect();
    }
    if on(sel, "NF009") {
        name = name
            .chars()
            .filter_map(|c| match invisible_info(c) {
                Some((_, InvisibleAction::Remove)) => None,
                Some((_, InvisibleAction::ReplaceWithSpace)) => Some(' '),
                None => Some(c),
            })
            .collect();
    }
    if on(sel, "NF008") {
        name = to_nfc(&name);
    }
    if on(sel, "NF005") {
        name = name.trim_start_matches(' ').to_string();
    }
    if on(sel, "NF004") {
        name = name.trim_end_matches([' ', '.']).to_string();
    }
    if on(sel, "NF010") && (name.len() > COMPONENT_LIMIT || utf16_len(&name) > COMPONENT_LIMIT) {
        name = truncate_component(&name);
    }
    // NF001 runs last: every stage above (control/space stripping, even
    // truncation) can expose a reserved stem that was not there before.
    if on(sel, "NF001") {
        let stem = windows_stem(&name);
        if is_reserved_stem(stem) {
            if name.len() >= COMPONENT_LIMIT || utf16_len(&name) >= COMPONENT_LIMIT {
                // No room for the `_`: shorten the stem by one character
                // instead, which also de-reserves it.
                let shorter: String = stem.chars().take(stem.chars().count() - 1).collect();
                name = format!("{}{}", shorter, &name[stem.len()..]);
            } else {
                let stem = stem.to_string();
                name = format!("{}_{}", stem, &name[stem.len()..]);
            }
        }
    }
    if name.is_empty() {
        name.push('_');
    }
    name
}

/// One planned rename inside `dir` (relative display path of the parent).
pub struct Rename {
    pub dir: String,
    pub dir_path: PathBuf,
    pub from: std::ffi::OsString,
    pub from_display: String,
    pub to: String,
    /// The check IDs that motivated this rename, in catalog order.
    pub reasons: Vec<&'static str>,
}

/// Bump `name` with `-2`, `-3`, ... before the extension until its
/// collision key is free, re-truncating if the suffix blows the budget.
fn dedupe(name: &str, taken: &[String]) -> String {
    let is_free = |cand: &str, taken: &[String]| !taken.contains(&casefold_key(cand));
    if is_free(name, taken) {
        return name.to_string();
    }
    let (stem, ext) = split_extension(name);
    for n in 2.. {
        let mut cand = format!("{stem}-{n}{ext}");
        if cand.len() > COMPONENT_LIMIT || utf16_len(&cand) > COMPONENT_LIMIT {
            cand = truncate_component(&cand);
        }
        if is_free(&cand, taken) {
            return cand;
        }
    }
    unreachable!("the integers ran out");
}

/// Plan renames for the sorted entries of one directory. Returns
/// `(entry index, new name)` pairs.
pub fn plan_dir(entries: &[Entry], sel: &Selection) -> Vec<(usize, String)> {
    let sanitized: Vec<String> = entries
        .iter()
        .map(|e| sanitize_name(&e.name, sel))
        .collect();
    let collisions_on = on(sel, "NF006") || on(sel, "NF007");

    // Pass 1: entries whose name is already clean keep it and claim its key
    // — unless an earlier clean sibling claimed the same key (a collision).
    let mut taken: Vec<String> = Vec::new();
    let mut needs_new: Vec<usize> = Vec::new();
    for (i, entry) in entries.iter().enumerate() {
        if sanitized[i].as_bytes() == entry.name.as_slice() {
            let key = casefold_key(&sanitized[i]);
            if collisions_on && taken.contains(&key) {
                needs_new.push(i); // clean name, but it collides
            } else {
                taken.push(key);
            }
        } else {
            needs_new.push(i);
        }
    }
    needs_new.sort_unstable();

    // Pass 2: give every renamed entry a key-free name.
    let mut plan = Vec::new();
    for i in needs_new {
        let target = dedupe(&sanitized[i], &taken);
        taken.push(casefold_key(&target));
        plan.push((i, target));
    }
    plan
}

/// Plan renames for a whole tree. The plan is ordered deepest-first so that
/// applying it top to bottom never invalidates a later entry's path.
pub fn plan(root: &Path, sel: &Selection) -> io::Result<Vec<Rename>> {
    let walk = walk(root, sel.max_files)?;
    let mut renames: Vec<Rename> = Vec::new();
    for dir in &walk.dirs {
        for (idx, to) in plan_dir(&dir.entries, sel) {
            let entry = &dir.entries[idx];
            let mut reasons: Vec<&'static str> = check_component(&entry.name)
                .iter()
                // Advisory checks (NF011/NF012) carry no fix and cannot
                // motivate a rename.
                .filter(|f| sel.is_enabled(f.check) && f.fix.is_some())
                .map(|f| f.check.id)
                .collect();
            if reasons.is_empty() {
                // A clean name renamed purely to resolve a sibling collision.
                reasons.push("NF006/NF007");
            }
            renames.push(Rename {
                dir: dir.rel.clone(),
                dir_path: dir.rel_path.clone(),
                from: entry.os_name.clone(),
                from_display: crate::checks::display_name(&entry.name),
                to,
                reasons,
            });
        }
    }
    // Deepest directories first; ties broken by name for determinism.
    renames.sort_by(|a, b| {
        let depth = |r: &Rename| r.dir.split('/').filter(|s| !s.is_empty()).count();
        depth(b)
            .cmp(&depth(a))
            .then_with(|| a.dir.cmp(&b.dir))
            .then_with(|| a.from_display.cmp(&b.from_display))
    });
    Ok(renames)
}

/// Apply a plan. Refuses to overwrite: if a target somehow exists (the tree
/// changed between plan and apply), that rename fails the whole run.
pub fn apply(root: &Path, renames: &[Rename]) -> io::Result<()> {
    for r in renames {
        let from = root.join(&r.dir_path).join(&r.from);
        let to = root.join(&r.dir_path).join(&r.to);
        if to.symlink_metadata().is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!(
                    "refusing to overwrite existing `{}` — tree changed since planning?",
                    to.display()
                ),
            ));
        }
        std::fs::rename(&from, &to)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::fs;

    fn san(bytes: &[u8]) -> String {
        sanitize_name(bytes, &Selection::all())
    }

    fn entry(name: &[u8]) -> Entry {
        Entry {
            name: name.to_vec(),
            os_name: OsString::from(String::from_utf8_lossy(name).into_owned()),
            is_dir: false,
        }
    }

    #[test]
    fn sanitize_merges_every_fix() {
        assert_eq!(san(b" report: v2. "), "report- v2");
        assert_eq!(san(b"aux. draft: "), "aux_. draft-");
        assert_eq!(san("cafe\u{0301}\n.txt".as_bytes()), "caf\u{00E9}.txt");
        assert_eq!(san(b"a<b>c*d?.log"), "a(b)cd.log");
        assert_eq!(san(b"..."), "_");
        assert_eq!(san(b"caf\xE9 menu.txt"), "caf_ menu.txt");
    }

    #[test]
    fn sanitize_is_idempotent() {
        for name in [
            &b" aux: draft. "[..],
            "cafe\u{0301}.txt".as_bytes(),
            b"nul.tar.gz",
            b"ok.txt",
            "fee\u{200B}plan".as_bytes(),
        ] {
            let once = san(name);
            assert_eq!(san(once.as_bytes()), once, "not idempotent for {name:?}");
        }
    }

    #[test]
    fn sanitize_leaves_clean_names_alone() {
        for name in [
            "README.md",
            ".gitignore",
            "caf\u{00E9}.txt",
            "写真 2024.jpg",
        ] {
            assert_eq!(san(name.as_bytes()), name);
        }
    }

    #[test]
    fn sanitize_respects_selection() {
        // With only NF002 enabled, the trailing dot must survive.
        let sel = Selection::build(
            &[find_check("NF002").unwrap()],
            &[],
            &crate::rules::Target::ALL,
        );
        assert_eq!(sanitize_name(b"a:b.", &sel), "a-b.");
    }

    #[test]
    fn stage_order_catches_uncovered_reserved_names() {
        // "nul ." is not reserved as-is; after the trailing trim it becomes
        // "nul", which is. The reserved stage must run after the trim.
        assert_eq!(san(b"nul ."), "nul_");
        // Removing a control char can expose a reserved name too.
        assert_eq!(san(b"co\tn.txt"), "con_.txt");
    }

    #[test]
    fn dedupe_bumps_before_the_extension() {
        let taken = vec![casefold_key("report.txt"), casefold_key("report-2.txt")];
        assert_eq!(dedupe("report.txt", &taken), "report-3.txt");
        assert_eq!(dedupe("fresh.txt", &taken), "fresh.txt");
    }

    #[test]
    fn dedupe_is_case_and_normalization_insensitive() {
        let taken = vec![casefold_key("CAF\u{00C9}.txt")];
        // NFD lowercase collides with NFC uppercase: must bump.
        assert_eq!(dedupe("cafe\u{0301}.txt", &taken), "cafe\u{0301}-2.txt");
    }

    #[test]
    fn plan_dir_keeps_first_and_renames_collisions() {
        let entries = vec![
            entry(b"README.md"),
            entry(b"ReadMe.md"),
            entry(b"readme.md"),
        ];
        let plan = plan_dir(&entries, &Selection::all());
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0], (1, "ReadMe-2.md".to_string()));
        assert_eq!(plan[1], (2, "readme-3.md".to_string()));
    }

    #[test]
    fn plan_dir_never_steals_a_clean_siblings_name() {
        // "nul_" already exists and is clean; fixing "nul" must not take it.
        let entries = vec![entry(b"nul"), entry(b"nul_")];
        let plan = plan_dir(&entries, &Selection::all());
        assert_eq!(plan, vec![(0, "nul_-2".to_string())]);
    }

    #[test]
    fn plan_dir_two_dirty_names_converging_get_distinct_targets() {
        // Both sanitize to "b": first (byte order) wins, second bumps.
        let entries = vec![entry(b"b "), entry(b"b.")];
        let plan = plan_dir(&entries, &Selection::all());
        assert_eq!(plan, vec![(0, "b".to_string()), (1, "b-2".to_string())]);
    }

    #[test]
    fn plan_and_apply_a_real_tree() {
        let root = std::env::temp_dir().join(format!("namefence-fix-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("sub dir.")).unwrap();
        fs::write(root.join("sub dir./aux.txt"), "keep me").unwrap();
        fs::write(root.join("plain.txt"), "untouched").unwrap();

        let sel = Selection::all();
        let renames = plan(&root, &sel).unwrap();
        // Deepest first: the file inside "sub dir." renames before the dir.
        assert_eq!(renames.len(), 2);
        assert_eq!(renames[0].from_display, "aux.txt");
        assert_eq!(renames[0].to, "aux_.txt");
        assert_eq!(renames[0].reasons, ["NF001"]);
        assert_eq!(renames[1].from_display, "sub dir.");
        assert_eq!(renames[1].to, "sub dir");

        apply(&root, &renames).unwrap();
        assert_eq!(
            fs::read_to_string(root.join("sub dir/aux_.txt")).unwrap(),
            "keep me"
        );
        assert!(root.join("plain.txt").exists());
        // A second plan on the fixed tree is empty: convergence.
        assert!(plan(&root, &sel).unwrap().is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn apply_refuses_to_overwrite() {
        let root = std::env::temp_dir().join(format!("namefence-noclobber-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("report."), "dirty").unwrap();
        let renames = plan(&root, &Selection::all()).unwrap();
        assert_eq!(renames[0].to, "report");
        // Simulate a race: the target appears between plan and apply.
        fs::write(root.join("report"), "sniped").unwrap();
        let err = apply(&root, &renames).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(root.join("report")).unwrap(), "sniped");
        let _ = fs::remove_dir_all(&root);
    }
}
