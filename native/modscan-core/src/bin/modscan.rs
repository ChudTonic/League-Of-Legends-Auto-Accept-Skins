//! Standalone CLI front-end for `modscan-core`.
//!
//! Usage: `modscan <file.fantome> [more files...] [--json]`
//!
//! Exit code is the worst verdict seen across all scanned files: 0 (Clean),
//! 1 (Suspicious), 2 (Malicious) — so this composes into scripts/CI without
//! needing to parse output. A file that can't even be read counts as 2.

use std::env;
use std::fs;
use std::process::ExitCode;

use modscan_core::{scan_bytes, Verdict};

fn verdict_exit_code(verdict: Verdict) -> u8 {
    match verdict {
        Verdict::Clean => 0,
        Verdict::Suspicious => 1,
        Verdict::Malicious => 2,
    }
}

fn main() -> ExitCode {
    let mut json_mode = false;
    let mut paths: Vec<String> = Vec::new();
    for arg in env::args().skip(1) {
        if arg == "--json" {
            json_mode = true;
        } else {
            paths.push(arg);
        }
    }

    if paths.is_empty() {
        eprintln!("usage: modscan <file.fantome> [more files...] [--json]");
        return ExitCode::from(2);
    }

    let mut worst_exit: u8 = 0;
    let mut json_reports: Vec<serde_json::Value> = Vec::new();

    for path in &paths {
        match fs::read(path) {
            Ok(data) => {
                let report = scan_bytes(&data);
                worst_exit = worst_exit.max(verdict_exit_code(report.verdict));
                if json_mode {
                    json_reports.push(serde_json::json!({ "file": path, "report": report }));
                } else {
                    println!("=== {path} ===");
                    print!("{}", report.human_summary());
                    println!();
                }
            }
            // Can't scan what we can't read — treat as the worst outcome
            // rather than silently skipping it.
            Err(err) => {
                eprintln!("modscan: failed to read '{path}': {err}");
                worst_exit = worst_exit.max(2);
            }
        }
    }

    if json_mode {
        match serde_json::to_string_pretty(&json_reports) {
            Ok(s) => println!("{s}"),
            Err(err) => eprintln!("modscan: failed to serialize JSON output: {err}"),
        }
    }

    ExitCode::from(worst_exit)
}
