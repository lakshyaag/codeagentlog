//! Timestamp parsing and local-time formatting.

use anyhow::{Context, Result, bail};
use jiff::tz::TimeZone;
use jiff::{Timestamp, Zoned, civil};
use serde_json::Value;

/// RFC 3339 string or epoch milliseconds.
pub fn parse_ts(v: Option<&Value>) -> Option<Timestamp> {
    match v? {
        Value::String(s) => s.parse().ok(),
        Value::Number(n) => Timestamp::from_millisecond(n.as_i64()?).ok(),
        _ => None,
    }
}

fn local(ts: Timestamp) -> Zoned {
    ts.to_zoned(TimeZone::system())
}

pub fn datetime(ts: Timestamp) -> String {
    local(ts).strftime("%Y-%m-%d %H:%M").to_string()
}

pub fn date(ts: Timestamp) -> String {
    local(ts).strftime("%Y-%m-%d").to_string()
}

pub fn clock(ts: Timestamp) -> String {
    local(ts).strftime("%H:%M:%S").to_string()
}

/// `2026-10-08 08:21 → 09:40`, with the end date shown only if it differs.
pub fn span(start: Timestamp, end: Timestamp) -> String {
    let (s, e) = (local(start), local(end));
    let end_fmt = if s.date() == e.date() { "%H:%M" } else { "%Y-%m-%d %H:%M" };
    format!("{} → {}", s.strftime("%Y-%m-%d %H:%M"), e.strftime(end_fmt))
}

/// Compact age: `now`, `5m`, `3h`, `2d`, `3w`, `4mo`, `1y`.
pub fn age(ts: Timestamp, now: Timestamp) -> String {
    let secs = (now.as_second() - ts.as_second()).max(0);
    let (m, h, d) = (60, 3600, 86400);
    match secs {
        s if s < 2 * m => "now".into(),
        s if s < h => format!("{}m", s / m),
        s if s < d => format!("{}h", s / h),
        s if s < 14 * d => format!("{}d", s / d),
        s if s < 60 * d => format!("{}w", s / (7 * d)),
        s if s < 365 * d => format!("{}mo", s / (30 * d)),
        s => format!("{}y", s / (365 * d)),
    }
}

/// `--since`: `30m`, `2h`, `3d`, `2w`, or a date `2026-10-01` (local midnight).
pub fn parse_since(s: &str, now: Timestamp) -> Result<Timestamp> {
    if let Ok(d) = s.parse::<civil::Date>() {
        return Ok(d.at(0, 0, 0, 0).to_zoned(TimeZone::system())?.timestamp());
    }
    let split = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    let (num, unit) = s.split_at(split);
    let n: i64 = num.parse().with_context(|| format!("invalid --since {s:?} (try 3d, 12h, 2w, 2026-10-01)"))?;
    let secs = match unit {
        "s" => 1,
        "m" | "min" => 60,
        "h" => 3600,
        "d" => 86400,
        "w" => 7 * 86400,
        "mo" => 30 * 86400,
        "y" => 365 * 86400,
        _ => bail!("invalid --since unit in {s:?} (use m, h, d, w, mo, y)"),
    };
    Ok(Timestamp::from_second(now.as_second() - n * secs)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        let now = Timestamp::from_second(1_000_000).unwrap();
        assert_eq!(parse_since("2h", now).unwrap().as_second(), 1_000_000 - 7200);
        assert!(parse_since("2x", now).is_err());
        assert!(parse_since("2026-10-01", now).is_ok());
    }

    #[test]
    fn formats_age() {
        let now = Timestamp::from_second(10_000_000).unwrap();
        assert_eq!(age(now, now), "now");
        assert_eq!(age(Timestamp::from_second(10_000_000 - 3 * 86400).unwrap(), now), "3d");
    }
}
