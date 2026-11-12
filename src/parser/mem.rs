//! `/proc/meminfo` ayrıştırma.

use super::{ParseError, malformed};
use crate::model::MemInfo;

/// kB değerini bayta çevirir (taşmada doyar).
fn kb_to_bytes(kb: u64) -> u64 {
    kb.saturating_mul(1024)
}

/// `Anahtar:   123 kB` satırından değeri (bayt) okur.
fn value(rest: &str, line: &str) -> Result<u64, ParseError> {
    let num = rest
        .split_whitespace()
        .next()
        .ok_or_else(|| malformed("meminfo", line))?;
    let kb: u64 = num.parse().map_err(|_| malformed("meminfo", line))?;
    // VARSAYIM: okuduğumuz tüm anahtarlar kB cinsindendir.
    Ok(kb_to_bytes(kb))
}

/// `/proc/meminfo` çıktısını bayt cinsinden `MemInfo`'ya çevirir.
/// `MemTotal` zorunludur; swap alanları yoksa 0 kabul edilir.
pub fn parse_meminfo(input: &str) -> Result<MemInfo, ParseError> {
    if input.trim().is_empty() {
        return Err(ParseError::Empty);
    }
    let (mut total, mut avail, mut free, mut buffers, mut cached) = (None, None, None, None, None);
    let (mut swap_total, mut swap_free) = (None, None);

    for line in input.lines() {
        let Some((key, rest)) = line.split_once(':') else {
            continue;
        };
        let slot = match key.trim() {
            "MemTotal" => &mut total,
            "MemAvailable" => &mut avail,
            "MemFree" => &mut free,
            "Buffers" => &mut buffers,
            "Cached" => &mut cached,
            "SwapTotal" => &mut swap_total,
            "SwapFree" => &mut swap_free,
            _ => continue,
        };
        *slot = Some(value(rest, line)?);
    }

    let total = total.ok_or(ParseError::MissingField("MemTotal"))?;
    let available = match avail {
        Some(a) => a,
        // VARSAYIM: MemAvailable (kernel 3.14+) yoksa MemFree+Buffers+Cached ile yaklaşıklanır.
        None => free
            .ok_or(ParseError::MissingField("MemAvailable"))?
            .saturating_add(buffers.unwrap_or(0))
            .saturating_add(cached.unwrap_or(0)),
    };
    let swap_total = swap_total.unwrap_or(0);
    Ok(MemInfo {
        total,
        // VARSAYIM: tutarsız değerlerde (available > total) toplamla sınırlanır.
        available: available.min(total),
        swap_total,
        swap_free: swap_free.unwrap_or(0).min(swap_total),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const M: &str = include_str!("../../tests/fixtures/meminfo.txt");
    const OLD: &str = include_str!("../../tests/fixtures/meminfo_old_kernel.txt");
    const BAD: &str = include_str!("../../tests/fixtures/malformed_meminfo.txt");

    #[test]
    fn parses_modern() {
        let m = parse_meminfo(M).unwrap();
        assert_eq!(m.total, 16315288 * 1024);
        assert_eq!(m.available, 11876544 * 1024);
        assert_eq!(m.swap_total, 8388604 * 1024);
        assert_eq!(m.swap_free, 8101372 * 1024);
    }

    #[test]
    fn old_kernel_fallback() {
        let m = parse_meminfo(OLD).unwrap();
        assert_eq!(m.total, 1017256 * 1024);
        assert_eq!(m.available, (102400 + 51200 + 307200) * 1024);
        assert_eq!(m.swap_total, 524284 * 1024);
    }

    #[test]
    fn empty_garbage_malformed() {
        assert_eq!(parse_meminfo(""), Err(ParseError::Empty));
        assert_eq!(
            parse_meminfo("foo\nbar: baz\n"),
            Err(ParseError::MissingField("MemTotal"))
        );
        assert!(parse_meminfo(BAD).is_err());
        assert!(parse_meminfo("MemTotal:\n").is_err());
        assert!(parse_meminfo("MemTotal: -5 kB\n").is_err());
    }

    #[test]
    fn missing_available_and_free() {
        assert_eq!(
            parse_meminfo("MemTotal: 100 kB\n"),
            Err(ParseError::MissingField("MemAvailable"))
        );
    }

    #[test]
    fn no_swap_is_zero() {
        let m = parse_meminfo("MemTotal: 100 kB\nMemAvailable: 40 kB\n").unwrap();
        assert_eq!((m.swap_total, m.swap_free), (0, 0));
    }

    #[test]
    fn huge_numbers_saturate() {
        let m = parse_meminfo(&format!("MemTotal: {} kB\nMemAvailable: 1 kB\n", u64::MAX)).unwrap();
        assert_eq!(m.total, u64::MAX);
        assert!(parse_meminfo("MemTotal: 99999999999999999999999 kB\n").is_err());
    }

    #[test]
    fn inconsistent_values_clamped() {
        let m = parse_meminfo(
            "MemTotal: 10 kB\nMemAvailable: 50 kB\nSwapTotal: 1 kB\nSwapFree: 9 kB\n",
        )
        .unwrap();
        assert_eq!(m.available, m.total);
        assert_eq!(m.swap_free, m.swap_total);
    }

    #[test]
    fn truncated_line_without_unit() {
        let m = parse_meminfo("MemTotal: 100\nMemAvailable: 40").unwrap();
        assert_eq!(m.available, 40 * 1024);
    }
}
