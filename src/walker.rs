//! Deterministic filesystem walk.
//!
//! The walker visits directories depth-first in byte-sorted order and
//! returns, per directory, its sorted entries. Determinism matters twice:
//! collision checks attribute a group to its *first* name, and two runs on
//! the same tree must produce byte-identical reports. Symlinks are never
//! followed (their names are still checked); `.git` directories are skipped
//! because their contents are machine-managed and not synced by the tools
//! namefence cares about.

use std::ffi::OsString;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// One directory entry, with its name both as raw bytes (what the checks
/// consume) and as the `OsString` needed to touch the filesystem again.
pub struct Entry {
    pub name: Vec<u8>,
    pub os_name: OsString,
    pub is_dir: bool,
}

/// One visited directory: its path relative to the scan root (`""` for the
/// root itself, display form) plus sorted entries.
pub struct Dir {
    pub rel: String,
    pub rel_path: PathBuf,
    pub entries: Vec<Entry>,
}

/// The result of a walk.
pub struct Walk {
    pub dirs: Vec<Dir>,
    pub files: usize,
    pub dir_count: usize,
    /// True when `max_files` stopped the walk early; scan-wide statements
    /// (like collision absence) are then only partial.
    pub truncated: bool,
}

/// Raw bytes of an `OsStr` name. On Unix this is exact; elsewhere it falls
/// back to UTF-8 (non-Unicode names cannot occur on those platforms' APIs).
pub fn os_name_bytes(name: &std::ffi::OsStr) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        name.as_bytes().to_vec()
    }
    #[cfg(not(unix))]
    {
        name.to_string_lossy().into_owned().into_bytes()
    }
}

/// Walk `root`, visiting at most `max_files` entries.
pub fn walk(root: &Path, max_files: usize) -> io::Result<Walk> {
    let meta = fs::metadata(root)?;
    if !meta.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("`{}` is not a directory", root.display()),
        ));
    }

    let mut walk = Walk {
        dirs: Vec::new(),
        files: 0,
        dir_count: 0,
        truncated: false,
    };
    // Depth-first stack of (rel display, rel path) pairs, pushed in reverse
    // sorted order so pop() yields sorted order.
    let mut stack: Vec<(String, PathBuf)> = vec![(String::new(), PathBuf::new())];
    let mut seen = 0usize;

    while let Some((rel, rel_path)) = stack.pop() {
        let abs = root.join(&rel_path);
        let mut entries: Vec<Entry> = Vec::new();
        for item in fs::read_dir(&abs)? {
            let item = item?;
            let os_name = item.file_name();
            let name = os_name_bytes(&os_name);
            // file_type() does not follow symlinks: a link to a directory is
            // treated as a leaf, so cycles are impossible.
            let is_dir = item.file_type()?.is_dir();
            entries.push(Entry {
                name,
                os_name,
                is_dir,
            });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        let mut children: Vec<(String, PathBuf)> = Vec::new();
        for entry in &entries {
            if seen >= max_files {
                walk.truncated = true;
                break;
            }
            seen += 1;
            if entry.is_dir {
                walk.dir_count += 1;
                if entry.name == b".git" {
                    continue;
                }
                let child_rel = if rel.is_empty() {
                    String::from_utf8_lossy(&entry.name).into_owned()
                } else {
                    format!("{rel}/{}", String::from_utf8_lossy(&entry.name))
                };
                children.push((child_rel, rel_path.join(&entry.os_name)));
            } else {
                walk.files += 1;
            }
        }
        // Children were collected in sorted order; push reversed so the
        // depth-first pop() visits them smallest-first.
        while let Some(child) = children.pop() {
            stack.push(child);
        }

        walk.dirs.push(Dir {
            rel,
            rel_path,
            entries,
        });
        if walk.truncated {
            break;
        }
    }
    Ok(walk)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("namefence-walk-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn walk_is_sorted_and_complete() {
        let root = tempdir("sorted");
        fs::create_dir_all(root.join("b/inner")).unwrap();
        fs::create_dir_all(root.join("a")).unwrap();
        fs::write(root.join("z.txt"), "z").unwrap();
        fs::write(root.join("a/1.txt"), "1").unwrap();
        fs::write(root.join("b/inner/deep.txt"), "d").unwrap();

        let walk = walk(&root, 100_000).unwrap();
        let rels: Vec<&str> = walk.dirs.iter().map(|d| d.rel.as_str()).collect();
        assert_eq!(rels, ["", "a", "b", "b/inner"]);
        assert_eq!(walk.files, 3);
        assert_eq!(walk.dir_count, 3);
        assert!(!walk.truncated);
        // Entries of the root are sorted by byte value.
        let names: Vec<String> = walk.dirs[0]
            .entries
            .iter()
            .map(|e| String::from_utf8_lossy(&e.name).into_owned())
            .collect();
        assert_eq!(names, ["a", "b", "z.txt"]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn git_directories_are_not_descended() {
        let root = tempdir("git");
        fs::create_dir_all(root.join(".git/objects")).unwrap();
        fs::write(root.join(".git/objects/aux"), "x").unwrap();
        fs::write(root.join("real.txt"), "x").unwrap();

        let walk = walk(&root, 100_000).unwrap();
        assert!(walk.dirs.iter().all(|d| !d.rel.starts_with(".git")));
        // .git itself is still listed as an entry of the root (its NAME is
        // checkable), its contents are not.
        assert!(walk.dirs[0].entries.iter().any(|e| e.name == b".git"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn max_files_truncates_honestly() {
        let root = tempdir("cap");
        for i in 0..20 {
            fs::write(root.join(format!("f{i:02}.txt")), "x").unwrap();
        }
        let walk = walk(&root, 5).unwrap();
        assert!(walk.truncated);
        assert_eq!(walk.files, 5);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn walking_a_file_is_an_error() {
        let root = tempdir("file");
        let f = root.join("x.txt");
        fs::write(&f, "x").unwrap();
        assert!(walk(&f, 10).is_err());
        let _ = fs::remove_dir_all(&root);
    }
}
