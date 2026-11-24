//! `roadmap` — verify every goal in `ROADMAP.md` whose phase has been reached.
//!
//! This is the "checks everything that is done" gate: it parses the roadmap,
//! runs each goal's `check:` command, and fails if a goal marked `done` or
//! `in-progress` does not actually pass. Goals marked `planned`/`blocked` are
//! reported but not run, so the file can describe future work without failing
//! CI today.

use std::process::{Command, ExitCode, Stdio};

use anyhow::{Context, Result};

#[derive(Debug)]
struct Goal {
    id: String,
    title: String,
    status: String,
    phase: u32,
    check: Option<String>,
}

fn main() -> ExitCode {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "ROADMAP.md".to_string());
    let raw = match std::fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("roadmap: cannot read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let goals = match parse(&raw) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("roadmap: parse error: {e}");
            return ExitCode::FAILURE;
        }
    };

    println!("roadmap: {} goals in {path}", goals.len());
    let mut failures = Vec::new();
    for g in &goals {
        let verdict = match g.status.as_str() {
            "done" | "in-progress" => {
                let Some(cmd) = g.check.as_ref() else {
                    eprintln!(
                        "  {}: '{}' has status {} but no check",
                        g.id, g.title, g.status
                    );
                    failures.push(format!("{} (no check defined)", g.id));
                    continue;
                };
                match run_check(cmd) {
                    Ok(()) => "PASS",
                    Err(e) => {
                        failures.push(format!("{} ({})", g.id, cmd));
                        eprintln!("  {}: FAIL\n     {e}", g.id);
                        continue;
                    }
                }
            }
            _ => "SKIP (not due)",
        };
        println!("  {:<8} [{}] {}", g.id, g.status, verdict);
    }

    if failures.is_empty() {
        println!("\nroadmap: all due goals pass");
        ExitCode::SUCCESS
    } else {
        eprintln!("\nroadmap: FAIL ({} goal(s) failing)", failures.len());
        for f in &failures {
            eprintln!("  - {f}");
        }
        ExitCode::FAILURE
    }
}

/// Parse `## GOAL <id>: <title>` blocks with `key: value` fields.
fn parse(raw: &str) -> Result<Vec<Goal>> {
    let mut goals = Vec::new();
    let mut cur: Option<Goal> = None;
    for line in raw.lines() {
        if let Some(rest) = line.strip_prefix("## GOAL ") {
            if let Some(g) = cur.take() {
                goals.push(g);
            }
            // "RO-1: Rail matcher core..."
            let (id, title) = rest
                .split_once(':')
                .map(|(a, b)| (a.trim().to_string(), b.trim().to_string()))
                .context("GOAL line missing ':'")?;
            cur = Some(Goal {
                id,
                title,
                status: String::new(),
                phase: 0,
                check: None,
            });
            continue;
        }
        let Some(g) = cur.as_mut() else { continue };
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim();
            let v = v.trim();
            match k {
                "status" => g.status = v.to_string(),
                "phase" => g.phase = v.parse().unwrap_or(0),
                "check" => g.check = Some(v.to_string()),
                _ => {}
            }
        }
    }
    if let Some(g) = cur.take() {
        goals.push(g);
    }
    if goals.is_empty() {
        anyhow::bail!("no GOAL blocks found");
    }
    Ok(goals)
}

fn run_check(cmd: &str) -> Result<()> {
    // Run the check through the platform's own shell. On Windows `bash -c`
    // cannot execute a `.exe` by absolute path, and mangles backslashes; use
    // cmd.exe there and bash elsewhere.
    let (prog, flag) = if cfg!(windows) {
        ("cmd", "/C")
    } else {
        ("bash", "-c")
    };
    let status = Command::new(prog)
        .arg(flag)
        .arg(cmd)
        .stdin(Stdio::null())
        .status()
        .with_context(|| format!("spawning {prog} for `{cmd}`"))?;
    if !status.success() {
        anyhow::bail!("command exited {}", status.code().unwrap_or(-1));
    }
    Ok(())
}
