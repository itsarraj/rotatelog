//! Parses human-friendly age strings (`"1d"`, `"12h"`, `"30m"`, `"45s"`) for
//! `--max-age`.

use anyhow::{anyhow, Result};
use std::time::Duration;

/// Parses a duration string into a [`Duration`]. Accepts an integer
/// followed by (case-insensitive) `s`, `m`, `h`, `d`, or `w`; a bare
/// integer is treated as seconds.
pub fn parse_duration(input: &str) -> Result<Duration> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("empty duration string"));
    }

    let (digits, seconds_per_unit) = match trimmed.chars().last() {
        Some(c) if c.eq_ignore_ascii_case(&'s') => (&trimmed[..trimmed.len() - 1], 1u64),
        Some(c) if c.eq_ignore_ascii_case(&'m') => (&trimmed[..trimmed.len() - 1], 60u64),
        Some(c) if c.eq_ignore_ascii_case(&'h') => (&trimmed[..trimmed.len() - 1], 3600u64),
        Some(c) if c.eq_ignore_ascii_case(&'d') => (&trimmed[..trimmed.len() - 1], 86400u64),
        Some(c) if c.eq_ignore_ascii_case(&'w') => (&trimmed[..trimmed.len() - 1], 604800u64),
        _ => (trimmed, 1u64),
    };

    let digits = digits.trim();
    if digits.is_empty() {
        return Err(anyhow!("duration '{input}' has a unit but no number"));
    }
    let value: u64 = digits.parse().map_err(|_| {
        anyhow!("'{input}' is not a valid duration (expected e.g. 1d, 12h, 30m, 45s)")
    })?;
    let total = value
        .checked_mul(seconds_per_unit)
        .ok_or_else(|| anyhow!("duration '{input}' overflows"))?;
    Ok(Duration::from_secs(total))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_seconds() {
        assert_eq!(parse_duration("45s").unwrap(), Duration::from_secs(45));
    }

    #[test]
    fn parses_minutes() {
        assert_eq!(parse_duration("30m").unwrap(), Duration::from_secs(1800));
    }

    #[test]
    fn parses_hours_case_insensitive() {
        assert_eq!(parse_duration("12H").unwrap(), Duration::from_secs(43200));
    }

    #[test]
    fn parses_days() {
        assert_eq!(parse_duration("1d").unwrap(), Duration::from_secs(86400));
    }

    #[test]
    fn parses_weeks() {
        assert_eq!(
            parse_duration("2w").unwrap(),
            Duration::from_secs(1_209_600)
        );
    }

    #[test]
    fn bare_number_is_seconds() {
        assert_eq!(parse_duration("90").unwrap(), Duration::from_secs(90));
    }

    #[test]
    fn rejects_empty_string() {
        assert!(parse_duration("").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_duration("a while").is_err());
    }
}
