//! Rendering: findings and rename plans as human text or stable JSON, and
//! the exit-code policy.
//!
//! The JSON shape is part of namefence's interface (CI pipelines parse it),
//! so it is rendered by hand with stable key order rather than through a
//! serializer dependency.

use crate::engine::{Finding, ScanResult};
use crate::fixname::Rename;
use crate::rules::Severity;

/// `--fail-on`: the lowest severity that makes the process exit 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailOn {
    Never,
    Info,
    Warning,
    Error,
}

impl FailOn {
    pub fn parse(s: &str) -> Option<FailOn> {
        match s {
            "never" => Some(FailOn::Never),
            "info" => Some(FailOn::Info),
            "warning" => Some(FailOn::Warning),
            "error" => Some(FailOn::Error),
            _ => None,
        }
    }
}

/// Should the run exit non-zero under the given policy?
pub fn should_fail(result: &ScanResult, fail_on: FailOn) -> bool {
    let threshold = match fail_on {
        FailOn::Never => return false,
        FailOn::Info => Severity::Info,
        FailOn::Warning => Severity::Warning,
        FailOn::Error => Severity::Error,
    };
    result
        .findings
        .iter()
        .any(|f| f.check.severity >= threshold)
}

/// Escape a string for a JSON literal (RFC 8259).
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

fn finding_text(f: &Finding) -> String {
    let mut line = format!(
        "{}: {} {} ({}): {}",
        f.path,
        f.check.severity.as_str(),
        f.check.id,
        f.check.name,
        f.message
    );
    if let Some(fix) = &f.fix {
        line.push_str(&format!("\n    fix: rename to `{fix}`"));
    }
    line
}

fn summary_line(result: &ScanResult) -> String {
    let s = &result.stats;
    let scanned = format!("{} file(s), {} directory(ies) scanned", s.files, s.dirs);
    let truncated = if s.truncated {
        " [walk truncated by --max-files; results are partial]"
    } else {
        ""
    };
    if result.findings.is_empty() {
        format!("findings: none — {scanned}{truncated}")
    } else {
        format!(
            "findings: {} — {} error(s), {} warning(s), {} info; {scanned}{truncated}",
            result.findings.len(),
            s.errors,
            s.warnings,
            s.infos
        )
    }
}

/// Full text report: one block per finding, then the summary line.
pub fn render_text(result: &ScanResult) -> String {
    let mut out = String::new();
    for f in &result.findings {
        out.push_str(&finding_text(f));
        out.push('\n');
    }
    if !result.findings.is_empty() {
        out.push('\n');
    }
    out.push_str(&summary_line(result));
    out.push('\n');
    out
}

/// Full JSON report with stable key order.
pub fn render_json(root: &str, result: &ScanResult) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"tool\": \"namefence\",\n");
    out.push_str(&format!("  \"version\": \"{}\",\n", crate::VERSION));
    out.push_str(&format!("  \"root\": \"{}\",\n", json_escape(root)));
    out.push_str("  \"findings\": [\n");
    for (i, f) in result.findings.iter().enumerate() {
        let fix = match &f.fix {
            Some(fix) => format!("\"{}\"", json_escape(fix)),
            None => "null".to_string(),
        };
        out.push_str(&format!(
            "    {{\"path\": \"{}\", \"check\": \"{}\", \"name\": \"{}\", \"severity\": \"{}\", \"message\": \"{}\", \"fix\": {}}}{}\n",
            json_escape(&f.path),
            f.check.id,
            f.check.name,
            f.check.severity.as_str(),
            json_escape(&f.message),
            fix,
            if i + 1 < result.findings.len() { "," } else { "" }
        ));
    }
    out.push_str("  ],\n");
    let s = &result.stats;
    out.push_str(&format!(
        "  \"stats\": {{\"files\": {}, \"dirs\": {}, \"errors\": {}, \"warnings\": {}, \"infos\": {}, \"truncated\": {}}}\n",
        s.files, s.dirs, s.errors, s.warnings, s.infos, s.truncated
    ));
    out.push_str("}\n");
    out
}

fn rename_path(r: &Rename) -> String {
    if r.dir.is_empty() {
        r.from_display.clone()
    } else {
        format!("{}/{}", r.dir, r.from_display)
    }
}

/// Text form of a rename plan (or of the applied result).
pub fn render_plan_text(renames: &[Rename], applied: bool) -> String {
    let mut out = String::new();
    for r in renames {
        let verb = if applied { "renamed" } else { "would rename" };
        out.push_str(&format!(
            "{verb} `{}` -> `{}`  ({})\n",
            rename_path(r),
            r.to,
            r.reasons.join(", ")
        ));
    }
    if renames.is_empty() {
        out.push_str("nothing to fix — every name is portable\n");
    } else if applied {
        out.push_str(&format!("applied {} rename(s)\n", renames.len()));
    } else {
        out.push_str(&format!(
            "planned {} rename(s) — re-run with --apply to perform them\n",
            renames.len()
        ));
    }
    out
}

/// JSON form of a rename plan.
pub fn render_plan_json(root: &str, renames: &[Rename], applied: bool) -> String {
    let mut out = String::new();
    out.push_str("{\n");
    out.push_str("  \"tool\": \"namefence\",\n");
    out.push_str(&format!("  \"version\": \"{}\",\n", crate::VERSION));
    out.push_str(&format!("  \"root\": \"{}\",\n", json_escape(root)));
    out.push_str(&format!("  \"applied\": {applied},\n"));
    out.push_str("  \"renames\": [\n");
    for (i, r) in renames.iter().enumerate() {
        let reasons: Vec<String> = r.reasons.iter().map(|s| format!("\"{s}\"")).collect();
        out.push_str(&format!(
            "    {{\"dir\": \"{}\", \"from\": \"{}\", \"to\": \"{}\", \"reasons\": [{}]}}{}\n",
            json_escape(&r.dir),
            json_escape(&r.from_display),
            json_escape(&r.to),
            reasons.join(", "),
            if i + 1 < renames.len() { "," } else { "" }
        ));
    }
    out.push_str("  ]\n}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::{scan_paths, Stats};

    fn result_with(paths: &[&str]) -> ScanResult {
        let lines: Vec<Vec<u8>> = paths.iter().map(|p| p.as_bytes().to_vec()).collect();
        scan_paths(&lines)
    }

    #[test]
    fn text_report_carries_fix_and_summary() {
        let result = result_with(&["aux.txt", "ok.txt"]);
        let text = render_text(&result);
        assert!(text.contains("aux.txt: error NF001 (windows-reserved-name)"));
        assert!(text.contains("fix: rename to `aux_.txt`"));
        assert!(text.contains("findings: 1 — 1 error(s), 0 warning(s), 0 info"));
    }

    #[test]
    fn clean_report_says_none() {
        let result = result_with(&["ok.txt"]);
        let text = render_text(&result);
        assert!(text.starts_with("findings: none"));
    }

    #[test]
    fn json_report_is_stable_and_escaped() {
        let result = result_with(&["say \"hi\".txt"]);
        let json = render_json(".", &result);
        assert!(json.contains("\"tool\": \"namefence\""));
        assert!(json.contains(&format!("\"version\": \"{}\"", crate::VERSION)));
        assert!(json.contains("\"check\": \"NF002\""));
        // The quote inside the filename must be escaped in path and message.
        assert!(json.contains("say \\\"hi\\\".txt"));
        assert!(json.contains("\"fix\": \"say 'hi'.txt\""));
        assert!(json.contains("\"stats\": {\"files\": 1"));
    }

    #[test]
    fn json_null_fix_for_advisory_findings() {
        let result = result_with(&["thumbs.db"]);
        let json = render_json(".", &result);
        assert!(json.contains("\"check\": \"NF012\""));
        assert!(json.contains("\"fix\": null"));
    }

    #[test]
    fn json_escape_covers_controls() {
        assert_eq!(json_escape("a\"b\\c\nd\u{1}"), "a\\\"b\\\\c\\nd\\u0001");
    }

    #[test]
    fn fail_on_thresholds() {
        let errors = result_with(&["aux.txt"]); // one error
        let warnings = result_with(&["thumbs.db"]); // one warning
        assert!(should_fail(&errors, FailOn::Warning));
        assert!(should_fail(&errors, FailOn::Error));
        assert!(!should_fail(&errors, FailOn::Never));
        assert!(should_fail(&warnings, FailOn::Warning));
        assert!(!should_fail(&warnings, FailOn::Error));
        let clean = ScanResult {
            findings: Vec::new(),
            stats: Stats {
                files: 0,
                dirs: 0,
                errors: 0,
                warnings: 0,
                infos: 0,
                truncated: false,
            },
        };
        assert!(!should_fail(&clean, FailOn::Info));
    }

    #[test]
    fn truncated_walks_are_labelled() {
        let mut result = result_with(&["ok.txt"]);
        result.stats.truncated = true;
        assert!(render_text(&result).contains("results are partial"));
        assert!(render_json(".", &result).contains("\"truncated\": true"));
    }
}
