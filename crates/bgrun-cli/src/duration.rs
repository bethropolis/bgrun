use std::str::FromStr;

use anyhow::{anyhow, Error};

/// Newtype wrapper for bgrun durations in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BgrunDuration(pub u64);

impl FromStr for BgrunDuration {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let ms = if let Some(n) = s.strip_suffix("ms") {
            n.parse::<u64>()
                .map_err(|_| anyhow!("invalid duration: {s:?}"))?
        } else if let Some(n) = s.strip_suffix('s') {
            n.parse::<u64>()
                .map(|n| n * 1_000)
                .map_err(|_| anyhow!("invalid duration: {s:?}"))?
        } else if let Some(n) = s.strip_suffix('m') {
            n.parse::<u64>()
                .map(|n| n * 60_000)
                .map_err(|_| anyhow!("invalid duration: {s:?}"))?
        } else if let Some(n) = s.strip_suffix('h') {
            n.parse::<u64>()
                .map(|n| n * 3_600_000)
                .map_err(|_| anyhow!("invalid duration: {s:?}"))?
        } else {
            s.parse::<u64>()
                .map(|n| n * 1_000)
                .map_err(|_| anyhow!("invalid duration: {s:?}"))?
        };
        Ok(BgrunDuration(ms))
    }
}

/// Newtype wrapper for bgrun byte sizes (e.g. log rotation thresholds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BgrunSize(pub u64);

impl FromStr for BgrunSize {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        // Longest suffixes first so "mb" doesn't parse as "b".
        for (suffix, mult) in [
            ("kb", 1024u64),
            ("mb", 1024 * 1024),
            ("gb", 1024 * 1024 * 1024),
            ("k", 1024u64),
            ("m", 1024 * 1024),
            ("g", 1024 * 1024 * 1024),
            ("b", 1u64),
        ] {
            // Case-insensitive ASCII suffix match; `get` keeps this safe
            // for non-ASCII input (returns None instead of panicking).
            let split = s.len().checked_sub(suffix.len()).and_then(|i| {
                s.get(i..)
                    .filter(|tail| tail.eq_ignore_ascii_case(suffix))
                    .map(|_| i)
            });
            if let Some(i) = split {
                let n = s[..i]
                    .trim()
                    .parse::<u64>()
                    .map_err(|_| anyhow!("invalid size: {s:?}"))?;
                return Ok(BgrunSize(n.saturating_mul(mult)));
            }
        }
        s.parse::<u64>()
            .map(BgrunSize)
            .map_err(|_| anyhow!("invalid size: {s:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration() {
        assert_eq!("500ms".parse::<BgrunDuration>().unwrap().0, 500);
        assert_eq!("5s".parse::<BgrunDuration>().unwrap().0, 5000);
        assert_eq!("2m".parse::<BgrunDuration>().unwrap().0, 120000);
        assert_eq!("1h".parse::<BgrunDuration>().unwrap().0, 3600000);
        assert_eq!("30".parse::<BgrunDuration>().unwrap().0, 30000);
        assert!("".parse::<BgrunDuration>().is_err());
        assert!("abc".parse::<BgrunDuration>().is_err());
    }

    #[test]
    fn test_parse_size() {
        assert_eq!("1024".parse::<BgrunSize>().unwrap().0, 1024);
        assert_eq!("50b".parse::<BgrunSize>().unwrap().0, 50);
        assert_eq!("1k".parse::<BgrunSize>().unwrap().0, 1024);
        assert_eq!("1K".parse::<BgrunSize>().unwrap().0, 1024);
        assert_eq!("50m".parse::<BgrunSize>().unwrap().0, 50 * 1024 * 1024);
        assert_eq!("50M".parse::<BgrunSize>().unwrap().0, 50 * 1024 * 1024);
        assert_eq!("50MB".parse::<BgrunSize>().unwrap().0, 50 * 1024 * 1024);
        assert_eq!("2g".parse::<BgrunSize>().unwrap().0, 2 * 1024 * 1024 * 1024);
        assert_eq!(" 10M ".parse::<BgrunSize>().unwrap().0, 10 * 1024 * 1024);
        assert!("".parse::<BgrunSize>().is_err());
        assert!("abc".parse::<BgrunSize>().is_err());
        assert!("10x".parse::<BgrunSize>().is_err());
    }
}
