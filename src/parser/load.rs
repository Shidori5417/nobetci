//! `/proc/loadavg` ayrıştırma.

use super::{ParseError, malformed};
use crate::model::LoadAvg;

/// `0.42 0.37 0.30 2/412 1893054` biçimindeki satırdan ilk üç değeri okur.
pub fn parse_loadavg(input: &str) -> Result<LoadAvg, ParseError> {
    if input.trim().is_empty() {
        return Err(ParseError::Empty);
    }
    let mut it = input.split_whitespace();
    let mut next = || -> Result<f64, ParseError> {
        let v: f64 = it
            .next()
            .ok_or_else(|| malformed("loadavg", input))?
            .parse()
            .map_err(|_| malformed("loadavg", input))?;
        // NaN/inf/negatif geçersiz.
        if v.is_finite() && v >= 0.0 {
            Ok(v)
        } else {
            Err(malformed("loadavg", input))
        }
    };
    Ok(LoadAvg {
        one: next()?,
        five: next()?,
        fifteen: next()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fixture() {
        let l = parse_loadavg(include_str!("../../tests/fixtures/loadavg.txt")).unwrap();
        assert_eq!((l.one, l.five, l.fifteen), (0.42, 0.37, 0.30));
    }

    #[test]
    fn rejects_bad() {
        assert_eq!(parse_loadavg(""), Err(ParseError::Empty));
        assert!(parse_loadavg("abc def ghi").is_err());
        assert!(parse_loadavg("0.1 0.2").is_err());
        assert!(parse_loadavg("NaN 0 0").is_err());
        assert!(parse_loadavg("inf 0 0").is_err());
        assert!(parse_loadavg("-1 0 0").is_err());
    }

    #[test]
    fn huge_value_ok_and_extra_fields_ignored() {
        let l = parse_loadavg("123456789.5 0.00 0.00").unwrap();
        assert_eq!(l.one, 123456789.5);
        assert!(parse_loadavg("1 2 3").is_ok());
    }
}
