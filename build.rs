//! Build script — sets BUILD_DATETIME environment variable at compile time.

fn main() {
    // Get current UTC datetime for version string
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();

    // Convert to human-readable UTC datetime (manual formatting to avoid chrono dependency)
    let secs_per_min = 60u64;
    let secs_per_hour = 3600u64;
    let secs_per_day = 86400u64;

    let days = now / secs_per_day;
    let remaining = now % secs_per_day;
    let hours = remaining / secs_per_hour;
    let minutes = (remaining % secs_per_hour) / secs_per_min;
    let _seconds = remaining % secs_per_min;

    // Calculate year/month/day from days since epoch (1970-01-01)
    let (year, month, day) = days_to_date(days);

    let datetime = format!(
        "{:04}-{:02}-{:02} {:02}:{:02} UTC",
        year, month, day, hours, minutes
    );

    println!("cargo:rustc-env=BUILD_DATETIME={}", datetime);
    // Force re-run on every build so BUILD_DATETIME is always current.
    // Without this, cargo caches the build script output and the date becomes stale.
    println!("cargo:rerun-if-changed=FORCE_REBUILD_ALWAYS");
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