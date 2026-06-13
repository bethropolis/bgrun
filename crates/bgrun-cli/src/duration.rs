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
}
