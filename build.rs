//! Build-time metadata capture for the `avrig` binary.
//!
//! Emits `cargo:rustc-env=` values consumed by `gateway::build_metadata()` and
//! `verbose_version_line()`:
//!
//! - `AVRIG_TARGET_TRIPLE` from `TARGET`
//! - `AVRIG_COMMIT_SHA` and `AVRIG_COMMIT_SHORT_SHA` from git (or `AVRIG_GIT_SHA`)
//! - `AVRIG_GIT_BRANCH` from git
//! - `AVRIG_GIT_DIRTY` from `git status --porcelain`
//! - `AVRIG_BUILD_TIMESTAMP` as UTC ISO-8601
//!
//! Every external command is optional: a missing git binary, a detached checkout, or a
//! source tarball without `.git` degrades to `unknown` instead of failing the build.

use std::path::Path;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    let target_triple = std::env::var("TARGET").unwrap_or_else(|_| "unknown".to_string());
    println!("cargo:rustc-env=AVRIG_TARGET_TRIPLE={target_triple}");

    let (commit_sha, commit_short_sha) = resolve_commit();
    let git_branch =
        git(&["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_else(|| "detached".to_string());
    let dirty = git(&["status", "--porcelain"]).is_some_and(|status| !status.trim().is_empty());
    let build_timestamp = format_timestamp_utc(epoch_seconds());

    println!("cargo:rustc-env=AVRIG_COMMIT_SHA={commit_sha}");
    println!("cargo:rustc-env=AVRIG_COMMIT_SHORT_SHA={commit_short_sha}");
    println!("cargo:rustc-env=AVRIG_GIT_BRANCH={git_branch}");
    println!("cargo:rustc-env=AVRIG_GIT_DIRTY={dirty}");
    println!("cargo:rustc-env=AVRIG_BUILD_TIMESTAMP={build_timestamp}");

    // Rebuild when the checked-out commit or the index changes, so the reported SHA and
    // dirty flag stay truthful. Skipped for source trees without git metadata.
    if Path::new(".git").exists() {
        println!("cargo:rerun-if-changed=.git/HEAD");
        println!("cargo:rerun-if-changed=.git/index");
    }
    println!("cargo:rerun-if-env-changed=AVRIG_GIT_SHA");
}

/// Resolve the full and 7-character commit SHA, honouring the `AVRIG_GIT_SHA` override
/// for builds from an exported source tree.
fn resolve_commit() -> (String, String) {
    if let Some(override_sha) = non_empty_env("AVRIG_GIT_SHA") {
        let short = short_sha(&override_sha);
        return (override_sha, short);
    }
    match git(&["rev-parse", "HEAD"]) {
        Some(sha) => {
            let short = short_sha(&sha);
            (sha, short)
        }
        None => ("unknown".to_string(), "unknown".to_string()),
    }
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

/// Run a git subcommand and return trimmed stdout, or `None` when git is absent, exits
/// non-zero (no repository, detached edge cases), or prints nothing.
fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn non_empty_env(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn format_timestamp_utc(secs: u64) -> String {
    let days = secs / 86400;
    let rem_secs = secs % 86400;
    let hours = rem_secs / 3600;
    let minutes = (rem_secs % 3600) / 60;
    let seconds = rem_secs % 60;

    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}Z")
}

/// Convert days since the Unix epoch to a proleptic Gregorian date (Howard Hinnant's
/// civil-from-days algorithm).
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    let d = (days as i64) + 719468;
    let era = if d >= 0 { d } else { d - 146096 } / 146097;
    let doe = d - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + (if mp < 10 { 3 } else { -9 });
    let year = y + (if month <= 2 { 1 } else { 0 });
    (year as u64, month as u64, day as u64)
}
