//! Scheduling (T0.2.6) per BRD §5.6 line 953 (`Consolidator::schedule`) and the
//! `src/scheduler.rs` file slot in the §5.6 lines 984-993 layout.
//!
//! This module holds the **pure timing arithmetic** — "given the wall clock now
//! and a configured `run_at` time-of-day, how long until the next run?" — kept
//! free of `tokio`, `async`, and side effects so it is exhaustively unit-
//! testable without sleeping or mocking a clock. [`Consolidator::schedule`]
//! (the headless library loop) and `vault-app`'s production scheduler both build
//! their sleep loop on top of [`duration_until_next_run`].
//!
//! BRD §5.6's `run_at` is "3:00 AM **local**" (line 944), so all arithmetic is
//! in local wall-clock time. Across a DST transition the computed duration can
//! be off by the offset shift for that one night — acceptable for a nightly
//! maintenance job at alpha scale; the next night self-corrects.

use std::time::Duration;

use chrono::{DateTime, Local, NaiveDateTime, NaiveTime};

/// The next wall-clock instant at `run_at` strictly after `now`.
///
/// If `run_at` is still ahead of `now` today, that is today's instant;
/// otherwise it rolls to `run_at` tomorrow. The comparison is **strict**, so a
/// call made exactly at `run_at` schedules the *next* day — never a zero-delay
/// immediate re-fire.
///
/// Pure and deterministic (no clock read), so the day-boundary / month-rollover
/// / exact-match edges are all directly unit-testable.
pub fn next_run_after(now: NaiveDateTime, run_at: NaiveTime) -> NaiveDateTime {
    let today = now.date().and_time(run_at);
    if today > now {
        today
    } else {
        (now.date() + chrono::Days::new(1)).and_time(run_at)
    }
}

/// How long to sleep from `now` until the next `run_at` in local time.
///
/// Thin wrapper over [`next_run_after`] that converts the local clock to a
/// [`std::time::Duration`] suitable for `tokio::time::sleep`. The delta is
/// always positive (strict comparison in `next_run_after`), so the
/// `unwrap_or(ZERO)` floor is defensive only — it never trips in practice and
/// keeps the function panic-free per the no-`unwrap`/`expect` rule.
pub fn duration_until_next_run(now: DateTime<Local>, run_at: NaiveTime) -> Duration {
    let now_naive = now.naive_local();
    let next = next_run_after(now_naive, run_at);
    (next - now_naive).to_std().unwrap_or(Duration::ZERO)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::NaiveDate;

    fn dt(y: i32, m: u32, d: u32, h: u32, min: u32, s: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(y, m, d)
            .unwrap()
            .and_hms_opt(h, min, s)
            .unwrap()
    }

    fn at(h: u32, m: u32) -> NaiveTime {
        NaiveTime::from_hms_opt(h, m, 0).unwrap()
    }

    #[test]
    fn run_at_later_today_schedules_today() {
        // 01:00 now, 03:00 target → today at 03:00.
        let now = dt(2026, 6, 14, 1, 0, 0);
        assert_eq!(next_run_after(now, at(3, 0)), dt(2026, 6, 14, 3, 0, 0));
    }

    #[test]
    fn run_at_already_passed_schedules_tomorrow() {
        // 05:00 now, 03:00 target → tomorrow at 03:00.
        let now = dt(2026, 6, 14, 5, 0, 0);
        assert_eq!(next_run_after(now, at(3, 0)), dt(2026, 6, 15, 3, 0, 0));
    }

    #[test]
    fn exactly_at_run_at_schedules_tomorrow_not_immediate() {
        // Strict comparison: firing exactly at 03:00:00 must NOT re-fire now.
        let now = dt(2026, 6, 14, 3, 0, 0);
        assert_eq!(next_run_after(now, at(3, 0)), dt(2026, 6, 15, 3, 0, 0));
    }

    #[test]
    fn one_second_before_run_at_schedules_today() {
        let now = dt(2026, 6, 14, 2, 59, 59);
        assert_eq!(next_run_after(now, at(3, 0)), dt(2026, 6, 14, 3, 0, 0));
    }

    #[test]
    fn rolls_over_month_and_year_boundary() {
        let now = dt(2026, 12, 31, 5, 0, 0);
        assert_eq!(next_run_after(now, at(3, 0)), dt(2027, 1, 1, 3, 0, 0));
    }

    #[test]
    fn duration_until_next_run_is_positive_and_within_a_day() {
        // Sanity on the Local wrapper: never zero, never more than 24h.
        let now = Local::now();
        let wait = duration_until_next_run(now, at(3, 0));
        assert!(wait > Duration::ZERO, "wait must be strictly positive");
        assert!(
            wait <= Duration::from_secs(24 * 60 * 60),
            "wait must be at most one day"
        );
    }

    #[test]
    fn duration_matches_next_run_after_delta() {
        // The wrapper's result equals next_run_after minus now, to the second.
        let now = Local::now();
        let next = next_run_after(now.naive_local(), at(3, 0));
        let wait = duration_until_next_run(now, at(3, 0));
        let expected = (next - now.naive_local()).num_seconds() as u64;
        // Allow a 1s slop for the two Local::now() reads straddling a tick.
        let got = wait.as_secs();
        assert!(
            got.abs_diff(expected) <= 1,
            "wait {got}s should match delta {expected}s"
        );
    }
}
