//! Small shared helpers: time-bound parsing and text sanitizing.

use anyhow::{Result, anyhow};
use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, TimeZone, Utc};

/// How a bare `YYYY-MM-DD` is anchored within its day.
#[derive(Clone, Copy, PartialEq)]
pub enum DayAnchor {
    /// Start of day (00:00:00) — inclusive *lower* bound (`--since`).
    Start,
    /// End of day (23:59:59) — inclusive *upper* bound (`--until`), so that
    /// `--until 2026-07-01` includes everything that happened on July 1.
    End,
}

/// Parse a `--since` bound (bare dates anchored at start of day). Accepts:
/// - relative: `30m`, `24h`, `7d`, `2w` (interpreted as *now minus* that span)
/// - absolute date: `2026-07-01` (midnight UTC)
/// - absolute datetime: `2026-07-01T13:00:00Z` (RFC3339) or `2026-07-01 13:00`
pub fn parse_time_bound(s: &str) -> Result<DateTime<Utc>> {
    parse_time_bound_at(s, Utc::now(), DayAnchor::Start)
}

/// Parse a `--until` bound: identical to [`parse_time_bound`] but a bare date is
/// anchored at end of day, making it a true inclusive upper bound.
pub fn parse_time_bound_until(s: &str) -> Result<DateTime<Utc>> {
    parse_time_bound_at(s, Utc::now(), DayAnchor::End)
}

/// Like [`parse_time_bound`], but with an explicit "now" for relative spans,
/// so callers (and tests) can be deterministic.
pub fn parse_time_bound_at(
    s: &str,
    now: DateTime<Utc>,
    anchor: DayAnchor,
) -> Result<DateTime<Utc>> {
    let s = s.trim();

    // Relative form: <number><unit>
    if let Some(dt) = parse_relative(s, now) {
        return Ok(dt);
    }

    // RFC3339 datetime.
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc));
    }

    // `YYYY-MM-DD HH:MM[:SS]`
    for fmt in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Ok(Utc.from_utc_datetime(&ndt));
        }
    }

    // Bare date -> start or end of day depending on which bound this is.
    if let Ok(date) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let ndt = match anchor {
            DayAnchor::Start => date.and_hms_opt(0, 0, 0),
            DayAnchor::End => date.and_hms_opt(23, 59, 59),
        }
        .ok_or_else(|| anyhow!("invalid date"))?;
        return Ok(Utc.from_utc_datetime(&ndt));
    }

    Err(anyhow!(
        "could not parse time '{s}' (try 7d, 24h, 2026-07-01, or an RFC3339 timestamp)"
    ))
}

fn parse_relative(s: &str, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let split = s.find(|c: char| !c.is_ascii_digit())?;
    if split == 0 {
        return None; // no leading digits
    }
    let (num_part, unit) = s.split_at(split);
    let n: i64 = num_part.parse().ok()?;
    // `try_*` keeps absurd inputs (e.g. `99999999999999w`) from panicking.
    let dur = match unit {
        "m" | "min" => Duration::try_minutes(n),
        "h" | "hr" => Duration::try_hours(n),
        "d" | "day" | "days" => Duration::try_days(n),
        "w" | "wk" => Duration::try_weeks(n),
        _ => None,
    }?;
    now.checked_sub_signed(dur)
}

/// Remove injected `<system-reminder>...</system-reminder>` blocks so they do
/// not create false matches when searching prose. Unterminated blocks are
/// dropped to the end of the string.
pub fn strip_reminders(text: &str) -> String {
    if !text.contains("<system-reminder>") {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<system-reminder>") {
        out.push_str(&rest[..start]);
        if let Some(end) = rest[start..].find("</system-reminder>") {
            rest = &rest[start + end + "</system-reminder>".len()..];
        } else {
            rest = "";
            break;
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, 7, 12, 0, 0).unwrap()
    }

    fn at(s: &str) -> DateTime<Utc> {
        parse_time_bound_at(s, now(), DayAnchor::Start).unwrap()
    }

    #[test]
    fn relative_time_bounds() {
        assert_eq!(
            at("24h"),
            Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).unwrap()
        );
        assert_eq!(
            at("7d"),
            Utc.with_ymd_and_hms(2026, 6, 30, 12, 0, 0).unwrap()
        );
        assert_eq!(
            at("2w"),
            Utc.with_ymd_and_hms(2026, 6, 23, 12, 0, 0).unwrap()
        );
        assert_eq!(
            at("30m"),
            Utc.with_ymd_and_hms(2026, 7, 7, 11, 30, 0).unwrap()
        );
    }

    #[test]
    fn absolute_time_bounds() {
        assert_eq!(
            at("2026-07-01"),
            Utc.with_ymd_and_hms(2026, 7, 1, 0, 0, 0).unwrap()
        );
        assert_eq!(
            at("2026-07-01T13:30:00Z"),
            Utc.with_ymd_and_hms(2026, 7, 1, 13, 30, 0).unwrap()
        );
        assert_eq!(
            at("2026-07-01 13:30"),
            Utc.with_ymd_and_hms(2026, 7, 1, 13, 30, 0).unwrap()
        );
    }

    #[test]
    fn until_bare_date_anchors_end_of_day() {
        // A bare date as an upper bound must include the whole day.
        assert_eq!(
            parse_time_bound_at("2026-07-01", now(), DayAnchor::End).unwrap(),
            Utc.with_ymd_and_hms(2026, 7, 1, 23, 59, 59).unwrap()
        );
        // An explicit datetime is unaffected by the anchor.
        assert_eq!(
            parse_time_bound_at("2026-07-01T13:30:00Z", now(), DayAnchor::End).unwrap(),
            Utc.with_ymd_and_hms(2026, 7, 1, 13, 30, 0).unwrap()
        );
    }

    #[test]
    fn bad_time_bounds_error() {
        assert!(parse_time_bound_at("tomorrow", now(), DayAnchor::Start).is_err());
        assert!(parse_time_bound_at("5x", now(), DayAnchor::Start).is_err());
        assert!(parse_time_bound_at("", now(), DayAnchor::Start).is_err());
        // Absurd relative span must error, not panic.
        assert!(parse_time_bound_at("99999999999999w", now(), DayAnchor::Start).is_err());
    }

    #[test]
    fn strip_reminders_removes_blocks() {
        let text = "before <system-reminder>secret nudge</system-reminder> after";
        assert_eq!(strip_reminders(text), "before  after");
        // unterminated reminder: drop to end
        assert_eq!(
            strip_reminders("keep <system-reminder>rest is gone"),
            "keep "
        );
        // no reminder: unchanged (and cheap early return)
        assert_eq!(strip_reminders("plain text"), "plain text");
        // multiple blocks
        assert_eq!(
            strip_reminders(
                "a<system-reminder>x</system-reminder>b<system-reminder>y</system-reminder>c"
            ),
            "abc"
        );
    }
}
