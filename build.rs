//! Build script — sets BUILD_DATETIME environment variable at compile time.
//!
//! Previously this script used `cargo:rerun-if-changed=FORCE_REBUILD_ALWAYS`
//! (a non-existent path) so the build script re-ran on every invocation,
//! killing the incremental compile cache (MINOR-1 in 2026-04-20 code review).
//!
//! The current approach re-runs only when the git commit or the
//! working-tree dirty-state changes:
//!   * `.git/HEAD`   — moves when the branch changes / HEAD advances.
//!   * `.git/index`  — updated on every `git add`, `commit`, `checkout`.
//!
//! When either changes, cargo re-executes this script; otherwise the
//! cached `BUILD_DATETIME` / `BUILD_GIT_SHA` env vars are reused, which
//! dramatically speeds up incremental rebuilds.
//!
//! Stamp format: `YYYY-MM-DD HH:MM UTC (sha=<short>[-dirty])`

fn main() {
    // BUILD-004: tolerate a misconfigured system clock (NTP-less VM,
    // dead CMOS battery). A clock before 1970 would otherwise panic
    // here and fail `cargo build` with no useful diagnostic.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_else(|_| std::time::Duration::from_secs(0))
        .as_secs();

    // Convert to human-readable UTC datetime (manual formatting to avoid chrono dependency)
    let secs_per_min = 60u64;
    let secs_per_hour = 3600u64;
    let secs_per_day = 86400u64;

    let days = now / secs_per_day;
    let remaining = now % secs_per_day;
    let hours = remaining / secs_per_hour;
    let minutes = (remaining % secs_per_hour) / secs_per_min;

    // Calculate year/month/day from days since epoch (1970-01-01)
    let (year, month, day) = days_to_date(days);

    let git_stamp = git_short_sha_with_dirty().unwrap_or_else(|| {
        // BUILD-012: surface the fallback to developers via a cargo warning
        // so a missing/broken `git` shows up in the build log instead of
        // silently embedding `sha=unknown` into release binaries.
        println!(
            "cargo:warning=git not found or repository state unreadable; \
             BUILD_GIT_SHA falls back to 'unknown'"
        );
        "unknown".to_string()
    });

    let datetime = format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC (sha={})",
        year, month, day, hours, minutes, git_stamp
    );

    println!("cargo:rustc-env=BUILD_DATETIME={}", datetime);
    println!("cargo:rustc-env=BUILD_GIT_SHA={}", git_stamp);

    // Re-run only on git state changes. Cargo invalidates the build-script
    // output when any of these files change (modification, creation, etc.).
    // Missing files are ignored — the stamp will simply fall back to
    // "unknown" and the build script caches that until a .git appears.
    println!("cargo:rerun-if-changed=.git/HEAD");
    println!("cargo:rerun-if-changed=.git/index");
    // BUILD-003: a fast-forward `git pull` on a tracked branch updates
    // `refs/heads/<branch>` without touching `.git/HEAD`, leaving the
    // embedded SHA stale until `cargo clean`. Watch the refs directory
    // and the packed-refs file so cargo re-runs this script after a pull
    // or a tag update.
    println!("cargo:rerun-if-changed=.git/refs/heads");
    println!("cargo:rerun-if-changed=.git/packed-refs");
}

/// Return `"<short7>"` or `"<short7>-dirty"` when `git` is available;
/// `None` when not in a git checkout or git is not on PATH. The dirty
/// flag includes both tracked-but-modified and untracked changes.
fn git_short_sha_with_dirty() -> Option<String> {
    let sha = std::process::Command::new("git")
        .args(["rev-parse", "--short=7", "HEAD"])
        .output()
        .ok()?;
    if !sha.status.success() {
        return None;
    }
    let sha = String::from_utf8(sha.stdout).ok()?.trim().to_string();
    if sha.is_empty() {
        return None;
    }

    // `git status --porcelain` returns empty output on a clean tree.
    // Untracked files count as dirty so adding a new file still refreshes
    // the stamp even before `git add`.
    let dirty = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .output()
        .ok()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);

    if dirty {
        Some(format!("{sha}-dirty"))
    } else {
        Some(sha)
    }
}

/// Convert days since Unix epoch to (year, month, day)
fn days_to_date(mut days: u64) -> (u64, u64, u64) {
    let mut year = 1970u64;

    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    let leap = is_leap(year);
    let month_days: [u64; 12] = [
        31,
        if leap { 29 } else { 28 },
        31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
    ];

    let mut month = 1u64;
    for &md in &month_days {
        if days < md {
            break;
        }
        days -= md;
        month += 1;
    }

    (year, month, days + 1)
}

fn is_leap(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}