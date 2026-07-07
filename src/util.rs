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

/// A snippet of text centered on a match, with a few lines of surrounding
/// context, capped at `max_chars` and marked with `…` where it was cut.
///
/// `start`/`end` are the byte range of the match within `text`. The window is
/// expanded by `ctx_lines` lines on each side, then, if still too long, capped
/// to `max_chars` centered on the match (on char boundaries).
pub fn build_snippet(
    text: &str,
    start: usize,
    end: usize,
    ctx_lines: usize,
    max_chars: usize,
) -> String {
    let line_start = text[..start].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let line_end = text[end..]
        .find('\n')
        .map(|i| end + i)
        .unwrap_or(text.len());

    let mut win_start = line_start;
    for _ in 0..ctx_lines {
        if win_start == 0 {
            break;
        }
        win_start = text[..win_start - 1]
            .rfind('\n')
            .map(|i| i + 1)
            .unwrap_or(0);
    }
    let mut win_end = line_end;
    for _ in 0..ctx_lines {
        if win_end >= text.len() {
            break;
        }
        win_end = text[win_end + 1..]
            .find('\n')
            .map(|i| win_end + 1 + i)
            .unwrap_or(text.len());
    }

    let mut window = &text[win_start..win_end];
    let mut prefix = win_start > 0;
    let mut suffix = win_end < text.len();

    if window.chars().count() > max_chars {
        let match_mid = (start + end) / 2;
        let half = max_chars / 2;
        let mut lo = match_mid.saturating_sub(half).max(win_start);
        let mut hi = (match_mid + half).min(win_end);
        while !text.is_char_boundary(lo) && lo > win_start {
            lo -= 1;
        }
        while !text.is_char_boundary(hi) && hi < win_end {
            hi += 1;
        }
        window = &text[lo..hi];
        prefix = prefix || lo > win_start;
        suffix = suffix || hi < win_end;
    }

    let mut out = String::new();
    if prefix {
        out.push('…');
    }
    out.push_str(window.trim_matches('\n'));
    if suffix {
        out.push('…');
    }
    out
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
    fn snippet_centers_and_marks_truncation() {
        let text = "line one\nline two has the needle here\nline three\nline four";
        let idx = text.find("needle").unwrap();
        let s = build_snippet(text, idx, idx + 6, 1, 200);
        assert!(s.contains("needle"));
        assert!(s.contains("line one"));

        // Long single line is centered and ellipsized around the match.
        let long = format!("{}NEEDLE{}", "a".repeat(500), "b".repeat(500));
        let mi = long.find("NEEDLE").unwrap();
        let s2 = build_snippet(&long, mi, mi + 6, 2, 60);
        assert!(s2.contains("NEEDLE"));
        assert!(s2.starts_with('…') && s2.ends_with('…'));
        assert!(s2.chars().count() <= 64);
    }

    #[test]
    fn snippet_is_multibyte_safe() {
        let text = "☃☃☃ needle ☃☃☃ padding ".repeat(20);
        let idx = text.find("needle").unwrap();
        let s = build_snippet(&text, idx, idx + 6, 2, 40);
        assert!(s.contains("needle"));
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
