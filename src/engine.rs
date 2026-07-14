//! The scan engine: glue between the walker and the pure checks.
//!
//! Produces one flat, deterministically-ordered list of findings for a
//! tree (`scan`) or for a list of paths piped in on stdin (`scan_paths`),
//! honoring the check selection (`--only` / `--skip` / `--targets`).

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

use crate::checks::{check_component, check_path_length, check_siblings};
use crate::rules::{Check, Severity, Target, CHECKS};
use crate::unicode::utf16_len;
use crate::walker::walk;

/// Which checks run, plus the numeric budgets.
pub struct Selection {
    /// `enabled[i]` corresponds to `CHECKS[i]`.
    pub enabled: Vec<bool>,
    pub max_path: usize,
    pub max_files: usize,
}

impl Selection {
    /// Everything on, default budgets.
    pub fn all() -> Selection {
        Selection {
            enabled: vec![true; CHECKS.len()],
            max_path: DEFAULT_MAX_PATH,
            max_files: DEFAULT_MAX_FILES,
        }
    }

    /// Build from `--only`, `--skip` and `--targets`. `only`/`skip` entries
    /// have already been resolved to catalog checks by the CLI.
    pub fn build(
        only: &[&'static Check],
        skip: &[&'static Check],
        targets: &[Target],
    ) -> Selection {
        let mut sel = Selection::all();
        for (i, check) in CHECKS.iter().enumerate() {
            let mut on = check.targets.iter().any(|t| targets.contains(t));
            if !only.is_empty() {
                on = on && only.iter().any(|c| c.id == check.id);
            }
            if skip.iter().any(|c| c.id == check.id) {
                on = false;
            }
            sel.enabled[i] = on;
        }
        sel
    }

    pub fn is_enabled(&self, check: &Check) -> bool {
        CHECKS
            .iter()
            .position(|c| c.id == check.id)
            .map(|i| self.enabled[i])
            .unwrap_or(false)
    }
}

pub const DEFAULT_MAX_PATH: usize = 240;
pub const DEFAULT_MAX_FILES: usize = 200_000;

/// A finding bound to a path.
pub struct Finding {
    /// Path relative to the scan root, display form (`/`-separated).
    pub path: String,
    pub check: &'static Check,
    pub message: String,
    pub fix: Option<String>,
}

pub struct Stats {
    pub files: usize,
    pub dirs: usize,
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub truncated: bool,
}

pub struct ScanResult {
    pub findings: Vec<Finding>,
    pub stats: Stats,
}

fn join_rel(dir: &str, name: &[u8]) -> String {
    let name = String::from_utf8_lossy(name);
    if dir.is_empty() {
        name.into_owned()
    } else {
        format!("{dir}/{name}")
    }
}

fn tally(findings: &[Finding], files: usize, dirs: usize, truncated: bool) -> Stats {
    let mut stats = Stats {
        files,
        dirs,
        errors: 0,
        warnings: 0,
        infos: 0,
        truncated,
    };
    for f in findings {
        match f.check.severity {
            Severity::Error => stats.errors += 1,
            Severity::Warning => stats.warnings += 1,
            Severity::Info => stats.infos += 1,
        }
    }
    stats
}

/// Check every entry of one directory listing: per-name checks, sibling
/// collisions and (when the parent itself is still within budget) the path
/// length. `dir` is the directory's own relative display path.
fn check_listing(dir: &str, names: &[&[u8]], sel: &Selection, out: &mut Vec<Finding>) {
    for name in names {
        let path = join_rel(dir, name);
        for f in check_component(name) {
            if sel.is_enabled(f.check) {
                out.push(Finding {
                    path: path.clone(),
                    check: f.check,
                    message: f.message,
                    fix: f.fix,
                });
            }
        }
        // Path budget: skip once the parent is already over — flagging every
        // descendant of one over-long directory would bury the real signal.
        if utf16_len(dir) <= sel.max_path {
            if let Some(f) = check_path_length(&path, sel.max_path) {
                if sel.is_enabled(f.check) {
                    out.push(Finding {
                        path: path.clone(),
                        check: f.check,
                        message: f.message,
                        fix: f.fix,
                    });
                }
            }
        }
    }
    for hit in check_siblings(names) {
        if sel.is_enabled(hit.finding.check) {
            out.push(Finding {
                path: join_rel(dir, names[hit.index]),
                check: hit.finding.check,
                message: hit.finding.message,
                fix: hit.finding.fix,
            });
        }
    }
}

fn sort_findings(findings: &mut [Finding]) {
    findings.sort_by(|a, b| a.path.cmp(&b.path).then(a.check.id.cmp(b.check.id)));
}

/// Scan a directory tree.
pub fn scan(root: &Path, sel: &Selection) -> io::Result<ScanResult> {
    let walk = walk(root, sel.max_files)?;
    let mut findings = Vec::new();
    for dir in &walk.dirs {
        let names: Vec<&[u8]> = dir.entries.iter().map(|e| e.name.as_slice()).collect();
        check_listing(&dir.rel, &names, sel, &mut findings);
    }
    sort_findings(&mut findings);
    let stats = tally(&findings, walk.files, walk.dir_count, walk.truncated);
    Ok(ScanResult { findings, stats })
}

/// Check a list of relative paths (e.g. `git ls-files -z` output) without
/// touching the filesystem. Every distinct component of every path is
/// checked once; components are grouped by parent directory so collision
/// checks work across the listed set.
pub fn scan_paths(paths: &[Vec<u8>]) -> ScanResult {
    // parent display path -> sorted unique child names
    let mut dirs: BTreeMap<String, Vec<Vec<u8>>> = BTreeMap::new();
    let mut file_count = 0usize;
    for raw in paths {
        if raw.is_empty() {
            continue;
        }
        file_count += 1;
        let mut parent = String::new();
        for comp in raw.split(|&b| b == b'/') {
            if comp.is_empty() || comp == b"." {
                continue; // leading "./", doubled or trailing slashes
            }
            let children = dirs.entry(parent.clone()).or_default();
            if !children.iter().any(|c| c == comp) {
                children.push(comp.to_vec());
            }
            parent = join_rel(&parent, comp);
        }
    }

    let sel = Selection::all();
    let mut findings = Vec::new();
    for (dir, mut names) in dirs {
        names.sort();
        let refs: Vec<&[u8]> = names.iter().map(|n| n.as_slice()).collect();
        check_listing(&dir, &refs, &sel, &mut findings);
    }
    sort_findings(&mut findings);
    let stats = tally(&findings, file_count, 0, false);
    ScanResult { findings, stats }
}

/// Like `scan_paths` but honoring a selection (used by the CLI).
pub fn scan_paths_with(paths: &[Vec<u8>], sel: &Selection) -> ScanResult {
    let full = scan_paths(paths);
    let mut findings: Vec<Finding> = full
        .findings
        .into_iter()
        .filter(|f| sel.is_enabled(f.check))
        .collect();
    // Path-budget findings were computed with the default; recompute against
    // the selected budget by filtering on the recorded message is fragile —
    // instead re-derive them here when the budget differs.
    if sel.max_path != DEFAULT_MAX_PATH {
        findings.retain(|f| f.check.id != "NF011");
        for raw in paths {
            if raw.is_empty() {
                continue;
            }
            let display = String::from_utf8_lossy(raw).into_owned();
            if let Some(f) = check_path_length(&display, sel.max_path) {
                if sel.is_enabled(f.check) {
                    findings.push(Finding {
                        path: display.clone(),
                        check: f.check,
                        message: f.message,
                        fix: f.fix,
                    });
                }
            }
        }
        sort_findings(&mut findings);
    }
    let stats = tally(&findings, full.stats.files, 0, false);
    ScanResult { findings, stats }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn tempdir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("namefence-engine-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ids(result: &ScanResult) -> Vec<(&str, &str)> {
        result
            .findings
            .iter()
            .map(|f| (f.path.as_str(), f.check.id))
            .collect()
    }

    #[test]
    fn scan_finds_and_orders_everything() {
        let root = tempdir("scan");
        fs::create_dir_all(root.join("sub")).unwrap();
        fs::write(root.join("aux.txt"), "x").unwrap();
        fs::write(root.join("sub/README.md"), "x").unwrap();
        fs::write(root.join("sub/readme.md"), "x").unwrap();
        fs::write(root.join("z:report.csv"), "x").unwrap();

        let result = scan(&root, &Selection::all()).unwrap();
        assert_eq!(
            ids(&result),
            [
                ("aux.txt", "NF001"),
                ("sub/readme.md", "NF006"),
                ("z:report.csv", "NF002"),
            ]
        );
        assert_eq!(result.stats.errors, 3);
        assert_eq!(result.stats.files, 4);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn selection_by_target_and_skip() {
        let root = tempdir("select");
        fs::write(root.join("aux.txt"), "x").unwrap(); // NF001: windows, cloud
        fs::write(root.join("thumbs.db"), "x").unwrap(); // NF012: cloud only

        let only_macos = Selection::build(&[], &[], &[Target::Macos]);
        let result = scan(&root, &only_macos).unwrap();
        assert!(result.findings.is_empty(), "macos target must mute both");

        let cloud = Selection::build(&[], &[], &[Target::Cloud]);
        let result = scan(&root, &cloud).unwrap();
        assert_eq!(result.findings.len(), 2);

        let skip = Selection::build(
            &[],
            &[crate::rules::find_check("NF012").unwrap()],
            &Target::ALL,
        );
        let result = scan(&root, &skip).unwrap();
        assert_eq!(ids(&result), [("aux.txt", "NF001")]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn nested_deep_path_reports_shallowest_offender_only() {
        let mut sel = Selection::all();
        sel.max_path = 20;
        let root = tempdir("deep");
        fs::create_dir_all(root.join("a-very-long-directory/child")).unwrap();
        fs::write(root.join("a-very-long-directory/child/f.txt"), "x").unwrap();
        let result = scan(&root, &sel).unwrap();
        let nf011: Vec<&str> = result
            .findings
            .iter()
            .filter(|f| f.check.id == "NF011")
            .map(|f| f.path.as_str())
            .collect();
        // The directory itself (21 units > 20) blows the budget; its
        // descendants are implied, not spammed.
        assert_eq!(nf011, ["a-very-long-directory"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn scan_paths_groups_by_parent_for_collisions() {
        let paths: Vec<Vec<u8>> = vec![
            b"docs/Readme.md".to_vec(),
            b"docs/readme.md".to_vec(),
            b"src/readme.md".to_vec(), // different directory: no collision
            b"src/aux.rs".to_vec(),
        ];
        let result = scan_paths(&paths);
        assert_eq!(
            ids(&result),
            [("docs/readme.md", "NF006"), ("src/aux.rs", "NF001")]
        );
        assert_eq!(result.stats.files, 4);
    }

    #[test]
    fn scan_paths_checks_directory_components_once() {
        let paths: Vec<Vec<u8>> = vec![
            b"bad dir./a.txt".to_vec(),
            b"bad dir./b.txt".to_vec(),
            b"./normal/c.txt".to_vec(),
        ];
        let result = scan_paths(&paths);
        let nf004: Vec<&str> = result
            .findings
            .iter()
            .filter(|f| f.check.id == "NF004")
            .map(|f| f.path.as_str())
            .collect();
        assert_eq!(nf004, ["bad dir."], "directory flagged exactly once");
    }

    #[test]
    fn scan_paths_with_custom_budget() {
        let mut sel = Selection::all();
        sel.max_path = 10;
        let paths: Vec<Vec<u8>> = vec![b"a/very/deep/path.txt".to_vec()];
        let result = scan_paths_with(&paths, &sel);
        assert!(result.findings.iter().any(|f| f.check.id == "NF011"));
    }
}
