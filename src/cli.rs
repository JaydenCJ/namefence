//! Command-line interface: argument parsing (hand-rolled, std-only) and
//! command dispatch.
//!
//! Parsing is a pure function from arguments to a `Command` so the whole
//! surface is unit-testable without spawning processes; `run` performs the
//! I/O. Exit codes: 0 = clean, 1 = findings at or above `--fail-on`,
//! 2 = usage or I/O error.

use std::ffi::OsString;
use std::io::Read;
use std::path::PathBuf;

use crate::engine::{scan, scan_paths_with, Selection, DEFAULT_MAX_FILES, DEFAULT_MAX_PATH};
use crate::fixname::{apply, plan};
use crate::report::{
    render_json, render_plan_json, render_plan_text, render_text, should_fail, FailOn,
};
use crate::rules::{find_check, Check, Target, CHECKS};

const USAGE: &str = "namefence — lints filenames that will break on Windows, macOS, or cloud sync

USAGE:
    namefence <COMMAND> [OPTIONS] [PATH]

COMMANDS:
    check [PATH]       lint every name under PATH (default: .)
    fix [PATH]         plan collision-safe renames; --apply performs them
    stdin              check newline- or NUL-separated paths read from stdin
    checks             list the check catalog
    explain <CHECK>    the full story and fix recipe for one check
    help               print this help

OPTIONS:
    --format <text|json>       output format (default: text)
    --fail-on <SEV>            exit 1 at this severity or above:
                               error | warning | info | never (default: warning)
    --only <LIST>              run only these checks (IDs or names, comma-separated)
    --skip <LIST>              skip these checks
    --targets <LIST>           platforms to lint for, comma-separated:
                               windows, macos, linux, cloud (default: all)
    --max-path <N>             NF011 path budget in UTF-16 units (default: 240)
    --max-files <N>            stop walking after N entries (default: 200000)
    --apply                    (fix) perform the planned renames
    -0, --null                 (stdin) input is NUL-separated (git ls-files -z)
    -h, --help                 print help
    -V, --version              print version

EXIT CODES:
    0  no findings at or above --fail-on
    1  findings at or above --fail-on (or an unapplied fix plan)
    2  usage or I/O error
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Text,
    Json,
}

/// Options shared by `check`, `fix` and `stdin`.
pub struct Opts {
    pub format: Format,
    pub fail_on: FailOn,
    pub only: Vec<&'static Check>,
    pub skip: Vec<&'static Check>,
    pub targets: Vec<Target>,
    pub max_path: usize,
    pub max_files: usize,
}

impl Opts {
    fn selection(&self) -> Selection {
        let mut sel = Selection::build(&self.only, &self.skip, &self.targets);
        sel.max_path = self.max_path;
        sel.max_files = self.max_files;
        sel
    }
}

impl Default for Opts {
    fn default() -> Opts {
        Opts {
            format: Format::Text,
            fail_on: FailOn::Warning,
            only: Vec::new(),
            skip: Vec::new(),
            targets: Target::ALL.to_vec(),
            max_path: DEFAULT_MAX_PATH,
            max_files: DEFAULT_MAX_FILES,
        }
    }
}

pub enum Command {
    Check {
        path: PathBuf,
        opts: Opts,
    },
    Fix {
        path: PathBuf,
        opts: Opts,
        apply: bool,
    },
    Stdin {
        opts: Opts,
        null: bool,
    },
    Checks,
    Explain {
        key: String,
    },
    Help,
    Version,
}

fn parse_check_list(list: &str) -> Result<Vec<&'static Check>, String> {
    let mut out = Vec::new();
    for part in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match find_check(part) {
            Some(c) => out.push(c),
            None => return Err(format!("unknown check `{part}` (see `namefence checks`)")),
        }
    }
    if out.is_empty() {
        return Err("empty check list".to_string());
    }
    Ok(out)
}

fn parse_target_list(list: &str) -> Result<Vec<Target>, String> {
    let mut out = Vec::new();
    for part in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        match Target::parse(part) {
            Some(t) => out.push(t),
            None => {
                return Err(format!(
                    "unknown target `{part}` (expected windows, macos, linux or cloud)"
                ))
            }
        }
    }
    if out.is_empty() {
        return Err("empty target list".to_string());
    }
    Ok(out)
}

/// Parse the argument list (without argv[0]) into a command.
pub fn parse(args: &[OsString]) -> Result<Command, String> {
    let mut it = args.iter();
    let sub = match it.next() {
        None => return Ok(Command::Help),
        Some(s) => s,
    };
    let sub_str = sub.to_string_lossy();
    match sub_str.as_ref() {
        "-h" | "--help" | "help" => return Ok(Command::Help),
        "-V" | "--version" | "version" => return Ok(Command::Version),
        "checks" => return Ok(Command::Checks),
        "explain" => {
            let key = it
                .next()
                .ok_or("explain needs a check ID or name, e.g. `namefence explain NF006`")?;
            return Ok(Command::Explain {
                key: key.to_string_lossy().into_owned(),
            });
        }
        "check" | "fix" | "stdin" => {}
        other => {
            return Err(format!("unknown command `{other}` (see --help)"));
        }
    }

    let mut opts = Opts::default();
    let mut path: Option<PathBuf> = None;
    let mut apply_flag = false;
    let mut null_flag = false;

    fn value_of(flag: &str, it: &mut std::slice::Iter<'_, OsString>) -> Result<String, String> {
        it.next()
            .map(|v| v.to_string_lossy().into_owned())
            .ok_or(format!("{flag} needs a value"))
    }

    while let Some(arg) = it.next() {
        let arg_str = arg.to_string_lossy();
        match arg_str.as_ref() {
            "--format" => {
                opts.format = match value_of("--format", &mut it)?.as_str() {
                    "text" => Format::Text,
                    "json" => Format::Json,
                    other => return Err(format!("unknown format `{other}` (text or json)")),
                }
            }
            "--fail-on" => {
                let v = value_of("--fail-on", &mut it)?;
                opts.fail_on = FailOn::parse(&v).ok_or(format!(
                    "unknown --fail-on `{v}` (error, warning, info, never)"
                ))?;
            }
            "--only" => opts.only = parse_check_list(&value_of("--only", &mut it)?)?,
            "--skip" => opts.skip = parse_check_list(&value_of("--skip", &mut it)?)?,
            "--targets" => opts.targets = parse_target_list(&value_of("--targets", &mut it)?)?,
            "--max-path" => {
                let v = value_of("--max-path", &mut it)?;
                opts.max_path = v
                    .parse()
                    .map_err(|_| format!("--max-path needs a number, got `{v}`"))?;
            }
            "--max-files" => {
                let v = value_of("--max-files", &mut it)?;
                opts.max_files = v
                    .parse()
                    .map_err(|_| format!("--max-files needs a number, got `{v}`"))?;
            }
            "--apply" if sub_str == "fix" => apply_flag = true,
            "-0" | "--null" if sub_str == "stdin" => null_flag = true,
            s if s.starts_with('-') => {
                return Err(format!("unknown option `{s}` for `{sub_str}` (see --help)"));
            }
            _ => {
                if sub_str == "stdin" {
                    return Err("stdin reads paths from standard input, not arguments".to_string());
                }
                if path.is_some() {
                    return Err(format!("unexpected extra argument `{arg_str}`"));
                }
                path = Some(PathBuf::from(arg));
            }
        }
    }

    let path = path.unwrap_or_else(|| PathBuf::from("."));
    Ok(match sub_str.as_ref() {
        "check" => Command::Check { path, opts },
        "fix" => Command::Fix {
            path,
            opts,
            apply: apply_flag,
        },
        _ => Command::Stdin {
            opts,
            null: null_flag,
        },
    })
}

fn print_checks() {
    for c in CHECKS {
        let targets: Vec<&str> = c.targets.iter().map(|t| t.as_str()).collect();
        println!(
            "{} {:<7} {:<25} [{}]",
            c.id,
            c.severity.as_str(),
            c.name,
            targets.join(", ")
        );
        println!("      {}", c.summary);
    }
}

fn split_stdin(bytes: &[u8], null: bool) -> Vec<Vec<u8>> {
    let sep = if null { b'\0' } else { b'\n' };
    bytes
        .split(move |&b| b == sep)
        .map(|line| {
            // Tolerate CRLF input in newline mode.
            if !null && line.ends_with(b"\r") {
                line[..line.len() - 1].to_vec()
            } else {
                line.to_vec()
            }
        })
        .filter(|line| !line.is_empty())
        .collect()
}

/// Execute the CLI. Returns the process exit code.
pub fn run(args: &[OsString]) -> i32 {
    let command = match parse(args) {
        Ok(c) => c,
        Err(msg) => {
            eprintln!("namefence: {msg}");
            return 2;
        }
    };
    match command {
        Command::Help => {
            print!("{USAGE}");
            0
        }
        Command::Version => {
            println!("namefence {}", crate::VERSION);
            0
        }
        Command::Checks => {
            print_checks();
            0
        }
        Command::Explain { key } => match find_check(&key) {
            Some(c) => {
                let targets: Vec<&str> = c.targets.iter().map(|t| t.as_str()).collect();
                println!(
                    "{} ({}) — severity {} — breaks on: {}\n",
                    c.id,
                    c.name,
                    c.severity.as_str(),
                    targets.join(", ")
                );
                println!("{}", c.explain);
                0
            }
            None => {
                eprintln!("namefence: unknown check `{key}` (see `namefence checks`)");
                2
            }
        },
        Command::Check { path, opts } => {
            let sel = opts.selection();
            match scan(&path, &sel) {
                Ok(result) => {
                    match opts.format {
                        Format::Text => print!("{}", render_text(&result)),
                        Format::Json => {
                            print!("{}", render_json(&path.display().to_string(), &result))
                        }
                    }
                    i32::from(should_fail(&result, opts.fail_on))
                }
                Err(e) => {
                    eprintln!("namefence: {}: {e}", path.display());
                    2
                }
            }
        }
        Command::Stdin { opts, null } => {
            let mut buf = Vec::new();
            if let Err(e) = std::io::stdin().read_to_end(&mut buf) {
                eprintln!("namefence: reading stdin: {e}");
                return 2;
            }
            let paths = split_stdin(&buf, null);
            let sel = opts.selection();
            let result = scan_paths_with(&paths, &sel);
            match opts.format {
                Format::Text => print!("{}", render_text(&result)),
                Format::Json => print!("{}", render_json("<stdin>", &result)),
            }
            i32::from(should_fail(&result, opts.fail_on))
        }
        Command::Fix {
            path,
            opts,
            apply: do_apply,
        } => {
            let sel = opts.selection();
            let renames = match plan(&path, &sel) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("namefence: {}: {e}", path.display());
                    return 2;
                }
            };
            if do_apply {
                if let Err(e) = apply(&path, &renames) {
                    eprintln!("namefence: applying renames: {e}");
                    return 2;
                }
            }
            match opts.format {
                Format::Text => print!("{}", render_plan_text(&renames, do_apply)),
                Format::Json => print!(
                    "{}",
                    render_plan_json(&path.display().to_string(), &renames, do_apply)
                ),
            }
            // An unapplied, non-empty plan exits 1 so `namefence fix` can
            // gate CI the same way `check` does; an applied plan is success.
            i32::from(!do_apply && !renames.is_empty())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    /// `Command` intentionally does not implement Debug (it embeds catalog
    /// references), so unwrap the error arm by hand.
    fn err(list: &[&str]) -> String {
        match parse(&args(list)) {
            Err(e) => e,
            Ok(_) => panic!("expected `{list:?}` to be rejected"),
        }
    }

    #[test]
    fn no_arguments_means_help() {
        assert!(matches!(parse(&args(&[])).unwrap(), Command::Help));
        assert!(matches!(parse(&args(&["--help"])).unwrap(), Command::Help));
    }

    #[test]
    fn check_defaults() {
        let Command::Check { path, opts } = parse(&args(&["check"])).unwrap() else {
            panic!("expected check");
        };
        assert_eq!(path, PathBuf::from("."));
        assert_eq!(opts.format, Format::Text);
        assert_eq!(opts.fail_on, FailOn::Warning);
        assert_eq!(opts.max_path, DEFAULT_MAX_PATH);
        assert_eq!(opts.targets.len(), 4);
    }

    #[test]
    fn full_flag_soup_parses() {
        let Command::Check { path, opts } = parse(&args(&[
            "check",
            "--format",
            "json",
            "--fail-on",
            "error",
            "--only",
            "NF001,case-collision",
            "--skip",
            "NF012",
            "--targets",
            "windows,cloud",
            "--max-path",
            "120",
            "--max-files",
            "50",
            "some/dir",
        ]))
        .unwrap() else {
            panic!("expected check");
        };
        assert_eq!(path, PathBuf::from("some/dir"));
        assert_eq!(opts.format, Format::Json);
        assert_eq!(opts.fail_on, FailOn::Error);
        assert_eq!(opts.only.len(), 2);
        assert_eq!(opts.only[1].id, "NF006");
        assert_eq!(opts.skip[0].id, "NF012");
        assert_eq!(opts.targets, vec![Target::Windows, Target::Cloud]);
        assert_eq!(opts.max_path, 120);
        assert_eq!(opts.max_files, 50);
    }

    #[test]
    fn errors_are_specific() {
        assert!(err(&["frobnicate"]).contains("unknown command"));
        assert!(err(&["check", "--only", "NF999"]).contains("unknown check `NF999`"));
        assert!(err(&["check", "--targets", "solaris"]).contains("unknown target"));
        assert!(err(&["check", "--max-path", "many"]).contains("needs a number"));
        assert!(err(&["check", "--format"]).contains("needs a value"));
        assert!(err(&["check", "a", "b"]).contains("extra argument"));
        assert!(err(&["explain"]).contains("needs a check"));
    }

    #[test]
    fn apply_and_null_are_command_scoped() {
        assert!(matches!(
            parse(&args(&["fix", "--apply"])).unwrap(),
            Command::Fix { apply: true, .. }
        ));
        assert!(err(&["check", "--apply"]).contains("unknown option"));
        assert!(matches!(
            parse(&args(&["stdin", "-0"])).unwrap(),
            Command::Stdin { null: true, .. }
        ));
        assert!(err(&["check", "-0"]).contains("unknown option"));
    }

    #[test]
    fn stdin_splitting_modes() {
        assert_eq!(
            split_stdin(b"a.txt\nb.txt\r\n\nc/d.txt\n", false),
            vec![b"a.txt".to_vec(), b"b.txt".to_vec(), b"c/d.txt".to_vec()]
        );
        // NUL mode keeps \n and \r as name bytes — that is the point of -0.
        assert_eq!(
            split_stdin(b"a\nb\0c\0", true),
            vec![b"a\nb".to_vec(), b"c".to_vec()]
        );
    }
}
