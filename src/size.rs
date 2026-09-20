//! Parses human-friendly size strings (`"10M"`, `"512K"`, `"1G"`, a bare
//! byte count) the same way `--max-size` accepts them on the command line.

use anyhow::{anyhow, Result};

/// Parses a size string into a byte count. Accepts a bare integer (bytes),
/// or an integer followed by (case-insensitive) `K`, `M`, or `G`, using
/// binary (1024-based) multiples — the same convention `logrotate` itself
/// uses for `size 10M`.
pub fn parse_size(input: &str) -> Result<u64> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(anyhow!("empty size string"));
    }

    let (digits, multiplier) = match trimmed.chars().last() {
        Some(c) if c.eq_ignore_ascii_case(&'k') => (&trimmed[..trimmed.len() - 1], 1024u64),
        Some(c) if c.eq_ignore_ascii_case(&'m') => (&trimmed[..trimmed.len() - 1], 1024 * 1024),
        Some(c) if c.eq_ignore_ascii_case(&'g') => {
            (&trimmed[..trimmed.len() - 1], 1024 * 1024 * 1024)
        }
        Some(c) if c.eq_ignore_ascii_case(&'b') => (&trimmed[..trimmed.len() - 1], 1u64),
        _ => (trimmed, 1u64),
    };

    let digits = digits.trim();
    if digits.is_empty() {
        return Err(anyhow!("size '{input}' has a unit but no number"));
    }
    let value: u64 = digits
        .parse()
        .map_err(|_| anyhow!("'{input}' is not a valid size (expected e.g. 10M, 512K, 2048)"))?;
    value
        .checked_mul(multiplier)
        .ok_or_else(|| anyhow!("size '{input}' overflows a 64-bit byte count"))
}

/// Renders a byte count back into a compact human-readable string, used for
/// the summary line printed after a rotation (`"rotated app.log (12.4 KiB)"`).
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit_idx = 0;
    while value >= 1024.0 && unit_idx < UNITS.len() - 1 {
        value /= 1024.0;
        unit_idx += 1;
    }
    if unit_idx == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{value:.1} {}", UNITS[unit_idx])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_bytes() {
        assert_eq!(parse_size("2048").unwrap(), 2048);
    }

    #[test]
    fn parses_kilobytes() {
        assert_eq!(parse_size("10K").unwrap(), 10 * 1024);
    }

    #[test]
    fn parses_megabytes_case_insensitive() {
        assert_eq!(parse_size("5m").unwrap(), 5 * 1024 * 1024);
    }

    #[test]
    fn parses_gigabytes() {
        assert_eq!(parse_size("1G").unwrap(), 1024 * 1024 * 1024);
    }

    #[test]
    fn parses_explicit_bytes_suffix() {
        assert_eq!(parse_size("512b").unwrap(), 512);
    }

    #[test]
    fn rejects_empty_string() {
        assert!(parse_size("").is_err());
    }

    #[test]
    fn rejects_unit_with_no_number() {
        assert!(parse_size("M").is_err());
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_size("ten megabytes").is_err());
    }

    #[test]
    fn format_size_stays_in_bytes_under_1024() {
        assert_eq!(format_size(512), "512 B");
    }

    #[test]
    fn format_size_picks_kib() {
        assert_eq!(format_size(2048), "2.0 KiB");
    }

    #[test]
    fn format_size_picks_mib() {
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MiB");
    }
}
