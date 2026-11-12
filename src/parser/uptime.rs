//! `/proc/uptime` ayrıştırma.

use super::{ParseError, malformed};
use std::time::Duration;

/// `1234567.89 4567890.12` satırından ilk alanı (çalışma süresi) okur.
pub fn parse_uptime(input: &str) -> Result<Duration, ParseError> {
    let first = input.split_whitespace().next().ok_or(ParseError::Empty)?;
    let secs: f64 = first.parse().map_err(|_| malformed("uptime", input))?;
    Duration::try_from_secs_f64(secs).map_err(|_| malformed("uptime", input))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        let d = parse_uptime(include_str!("../../tests/fixtures/uptime.txt")).unwrap();
        assert_eq!(d.as_secs(), 1234567);
        assert!(d.subsec_millis() >= 889 && d.subsec_millis() <= 890);
    }

    #[test]
    fn rejects_bad() {
        assert_eq!(parse_uptime(""), Err(ParseError::Empty));
        assert_eq!(parse_uptime(" \n"), Err(ParseError::Empty));
        assert!(parse_uptime("abc 1").is_err());
        assert!(parse_uptime("-5 1").is_err());
        assert!(parse_uptime("NaN 1").is_err());
        assert!(parse_uptime("inf 1").is_err());
        assert!(parse_uptime("1e300 1").is_err());
    }

    #[test]
    fn single_field_ok() {
        assert_eq!(parse_uptime("42").unwrap(), Duration::from_secs(42));
    }
}
