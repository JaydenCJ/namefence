//! End-to-end tests that exercise the compiled `namefence` binary: check,
//! fix (plan and apply), stdin, checks, explain, JSON output, exit codes
//! and check selection. Every test builds its own fixture tree under a
//! temporary directory — offline, deterministic, no shared state.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_namefence")
}

fn run(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .output()
        .expect("failed to run namefence binary")
}

fn run_stdin(args: &[&str], input: &[u8]) -> Output {
    use std::io::Write;
    let mut child = Command::new(bin())
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn namefence binary");
    child.stdin.as_mut().unwrap().write_all(input).unwrap();
    child.wait_with_output().expect("failed to wait")
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn tempdir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("namefence-cli-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(root: &Path, rel: &str, content: &str) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// The demo tree from the README: one of several classic sync killers.
fn fixture(tag: &str) -> PathBuf {
    let root = tempdir(tag);
    write(&root, "aux.txt", "reserved");
    write(&root, "report:final.csv", "illegal char");
    write(&root, "notes. ", "trailing");
    write(&root, "docs/README.md", "first");
    write(&root, "docs/readme.md", "case twin");
    write(&root, "photos/cafe\u{0301}.jpg", "nfd");
    write(&root, "clean/plain.txt", "fine");
    root
}

#[test]
fn version_matches_manifest() {
    let out = run(&["--version"]);
    assert!(out.status.success());
    assert_eq!(
        stdout(&out).trim(),
        format!("namefence {}", env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn help_lists_commands_and_exits_zero() {
    for invocation in [vec!["--help"], vec!["help"], vec![]] {
        let out = run(&invocation);
        assert!(out.status.success());
        let text = stdout(&out);
        assert!(text.contains("COMMANDS:"));
        assert!(text.contains("EXIT CODES:"));
        for cmd in ["check", "fix", "stdin", "checks", "explain"] {
            assert!(text.contains(cmd), "help missing `{cmd}`");
        }
    }
}

#[test]
fn checks_catalog_lists_all_thirteen() {
    let out = run(&["checks"]);
    assert!(out.status.success());
    let text = stdout(&out);
    let count = text.lines().filter(|l| l.starts_with("NF0")).count();
    assert_eq!(count, 13);
    assert!(text.contains("windows-reserved-name"));
    assert!(text.contains("normalization-collision"));
    assert!(text.contains("[cloud]"));
}

#[test]
fn explain_accepts_id_and_name_rejects_unknown() {
    let by_id = run(&["explain", "NF008"]);
    assert!(by_id.status.success());
    assert!(stdout(&by_id).contains("NFC"));
    let by_name = run(&["explain", "non-nfc"]);
    assert_eq!(stdout(&by_name), stdout(&by_id));

    let bad = run(&["explain", "NF042"]);
    assert_eq!(bad.status.code(), Some(2));
    assert!(stderr(&bad).contains("unknown check"));
}

#[test]
fn check_finds_the_classics_and_exits_one() {
    let root = fixture("classics");
    let out = run(&["check", root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("aux.txt: error NF001 (windows-reserved-name)"));
    assert!(text.contains("fix: rename to `aux_.txt`"));
    assert!(text.contains("report:final.csv: error NF002"));
    assert!(text.contains("fix: rename to `report-final.csv`"));
    assert!(text.contains("notes. : error NF004"));
    assert!(text.contains("docs/readme.md: error NF006"));
    assert!(text.contains("collides with sibling `README.md`"));
    assert!(text.contains("photos/cafe\u{0301}.jpg: warning NF008"));
    assert!(text.contains("findings: 5 — 4 error(s), 1 warning(s), 0 info"));
    assert!(text.contains("7 file(s), 3 directory(ies) scanned"));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn clean_tree_exits_zero() {
    let root = tempdir("clean");
    write(&root, "src/main.rs", "x");
    write(&root, "README.md", "x");
    let out = run(&["check", root.to_str().unwrap()]);
    assert!(out.status.success(), "stderr: {}", stderr(&out));
    assert!(stdout(&out).starts_with("findings: none"));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn fail_on_policy_tunes_the_exit_code() {
    let root = tempdir("failon");
    write(&root, "photos/cafe\u{0301}.jpg", "nfd"); // warning only
    let path = root.to_str().unwrap();
    assert_eq!(run(&["check", path]).status.code(), Some(1));
    assert_eq!(
        run(&["check", "--fail-on", "error", path]).status.code(),
        Some(0)
    );
    assert_eq!(
        run(&["check", "--fail-on", "never", path]).status.code(),
        Some(0)
    );
    assert_eq!(
        run(&["check", "--fail-on", "info", path]).status.code(),
        Some(1)
    );
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn only_skip_and_targets_select_checks() {
    let root = fixture("select");
    let path = root.to_str().unwrap();

    let only = run(&["check", "--only", "NF001", path]);
    let text = stdout(&only);
    assert!(text.contains("NF001") && !text.contains("NF006"));

    let skip = run(&["check", "--skip", "NF001,NF002,NF004,NF006", path]);
    let text = stdout(&skip);
    assert!(!text.contains("NF001") && text.contains("NF008"));

    // Linux target: none of the fixture's problems break on pure Linux.
    let linux = run(&["check", "--targets", "linux", path]);
    assert_eq!(linux.status.code(), Some(0));
    assert!(stdout(&linux).starts_with("findings: none"));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn json_output_is_machine_readable() {
    let root = fixture("json");
    let out = run(&["check", "--format", "json", root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let json = stdout(&out);
    assert!(json.contains("\"tool\": \"namefence\""));
    assert!(json.contains(&format!("\"version\": \"{}\"", env!("CARGO_PKG_VERSION"))));
    assert!(json.contains(
        "\"check\": \"NF001\", \"name\": \"windows-reserved-name\", \"severity\": \"error\""
    ));
    assert!(json.contains("\"fix\": \"aux_.txt\""));
    assert!(json.contains("\"stats\": {\"files\": 7, \"dirs\": 3, \"errors\": 4, \"warnings\": 1, \"infos\": 0, \"truncated\": false}"));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn fix_plans_without_touching_and_applies_on_request() {
    let root = fixture("fix");
    let path = root.to_str().unwrap();

    let plan = run(&["fix", path]);
    assert_eq!(plan.status.code(), Some(1), "unapplied plan gates CI");
    let text = stdout(&plan);
    assert!(text.contains("would rename `aux.txt` -> `aux_.txt`  (NF001)"));
    assert!(text.contains("would rename `docs/readme.md` -> `readme-2.md`  (NF006/NF007)"));
    assert!(text.contains("re-run with --apply"));
    assert!(root.join("aux.txt").exists(), "plan must not rename");

    let apply = run(&["fix", "--apply", path]);
    assert_eq!(apply.status.code(), Some(0), "stderr: {}", stderr(&apply));
    let text = stdout(&apply);
    assert!(text.contains("renamed `aux.txt` -> `aux_.txt`"));
    assert!(text.contains("applied 5 rename(s)"));
    assert_eq!(
        fs::read_to_string(root.join("aux_.txt")).unwrap(),
        "reserved"
    );
    assert_eq!(
        fs::read_to_string(root.join("docs/readme-2.md")).unwrap(),
        "case twin"
    );
    assert_eq!(
        fs::read_to_string(root.join("photos/caf\u{00E9}.jpg")).unwrap(),
        "nfd"
    );

    // The fixed tree is clean and a second fix is a no-op: convergence.
    assert_eq!(run(&["check", path]).status.code(), Some(0));
    let again = run(&["fix", path]);
    assert_eq!(again.status.code(), Some(0));
    assert!(stdout(&again).contains("nothing to fix"));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn fix_json_plan_shape() {
    let root = tempdir("fixjson");
    write(&root, "nul.log", "x");
    let out = run(&["fix", "--format", "json", root.to_str().unwrap()]);
    let json = stdout(&out);
    assert!(json.contains("\"applied\": false"));
    assert!(json.contains(
        "{\"dir\": \"\", \"from\": \"nul.log\", \"to\": \"nul_.log\", \"reasons\": [\"NF001\"]}"
    ));
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn stdin_checks_git_ls_files_style_input() {
    let input = b"docs/Readme.md\ndocs/readme.md\nsrc/aux.rs\nclean.txt\n";
    let out = run_stdin(&["stdin"], input);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("docs/readme.md: error NF006"));
    assert!(text.contains("src/aux.rs: error NF001"));
    assert!(!text.contains("clean.txt:"));
    assert!(text.contains("4 file(s)"));
}

#[test]
fn stdin_null_mode_preserves_newlines_in_names() {
    // A filename containing a real newline arrives intact only with -0.
    let input = b"bad\nname.txt\0ok.txt\0";
    let out = run_stdin(&["stdin", "-0"], input);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(
        text.contains("NF003"),
        "control character finding expected: {text}"
    );
    assert!(text.contains("2 file(s)"));
}

#[test]
fn stdin_respects_max_path_budget() {
    let long = format!("{}/leaf.txt\n", "d".repeat(300));
    let over = run_stdin(&["stdin", "--max-path", "200"], long.as_bytes());
    assert!(stdout(&over).contains("NF011"));
    let under = run_stdin(&["stdin", "--max-path", "400"], long.as_bytes());
    assert!(!stdout(&under).contains("NF011"));
}

#[test]
fn io_and_usage_errors_exit_two() {
    let missing = run(&["check", "/definitely/not/a/dir"]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(!stderr(&missing).is_empty());

    let usage = run(&["check", "--format", "yaml"]);
    assert_eq!(usage.status.code(), Some(2));
    assert!(stderr(&usage).contains("unknown format"));

    let unknown = run(&["defragment"]);
    assert_eq!(unknown.status.code(), Some(2));
    assert!(stderr(&unknown).contains("unknown command"));
}

#[test]
fn git_dir_contents_are_ignored() {
    let root = tempdir("gitskip");
    write(&root, ".git/objects/aux", "not a real problem");
    write(&root, "real.txt", "x");
    let out = run(&["check", root.to_str().unwrap()]);
    assert!(out.status.success(), "{}", stdout(&out));
    let _ = fs::remove_dir_all(&root);
}

#[cfg(unix)]
#[test]
fn invalid_utf8_names_are_caught_end_to_end() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let root = tempdir("badutf8");
    fs::write(root.join(OsStr::from_bytes(b"caf\xE9 menu.txt")), "latin-1").unwrap();
    let out = run(&["check", root.to_str().unwrap()]);
    assert_eq!(out.status.code(), Some(1));
    let text = stdout(&out);
    assert!(text.contains("NF013"));
    assert!(text.contains("0xE9"));

    let fix = run(&["fix", "--apply", root.to_str().unwrap()]);
    assert!(fix.status.success(), "stderr: {}", stderr(&fix));
    assert!(root.join("caf_ menu.txt").exists());
    let _ = fs::remove_dir_all(&root);
}
