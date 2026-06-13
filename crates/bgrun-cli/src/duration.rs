use anyhow::{anyhow, Result};

/// Parses a duration string like "30s", "5m", "2h", "500ms" into milliseconds.
/// Bare numbers are treated as seconds.
pub fn parse_duration_ms(s: &str) -> Result<u64> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix("ms") {
        n.parse::<u64>()
            .map_err(|_| anyhow!("invalid duration: {s:?}"))
    } else if let Some(n) = s.strip_suffix('s') {
        n.parse::<u64>()
            .map(|n| n * 1_000)
            .map_err(|_| anyhow!("invalid duration: {s:?}"))
    } else if let Some(n) = s.strip_suffix('m') {
        n.parse::<u64>()
            .map(|n| n * 60_000)
            .map_err(|_| anyhow!("invalid duration: {s:?}"))
    } else if let Some(n) = s.strip_suffix('h') {
        n.parse::<u64>()
            .map(|n| n * 3_600_000)
            .map_err(|_| anyhow!("invalid duration: {s:?}"))
    } else {
        s.parse::<u64>()
            .map(|n| n * 1_000)
            .map_err(|_| anyhow!("invalid duration: {s:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_ms() {
        assert_eq!(parse_duration_ms("500ms").unwrap(), 500);
        assert_eq!(parse_duration_ms("5s").unwrap(), 5000);
        assert_eq!(parse_duration_ms("2m").unwrap(), 120000);
        assert_eq!(parse_duration_ms("1h").unwrap(), 3600000);
        assert_eq!(parse_duration_ms("30").unwrap(), 30000);
        assert!(parse_duration_ms("").is_err());
        assert!(parse_duration_ms("abc").is_err());
        assert!(parse_duration_ms("10x").is_err());
    }
}
