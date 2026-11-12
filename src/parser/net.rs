//! `/proc/net/dev` ayrıştırma ve bayt/sn hesabı.

use super::{ParseError, malformed};
use crate::model::NetDev;
use std::time::Duration;

/// Bir arayüzün anlık hızı (bayt/sn).
#[derive(Debug, Clone, PartialEq)]
pub struct NetRate {
    pub iface: String,
    pub rx_bps: f64,
    pub tx_bps: f64,
}

/// `/proc/net/dev` çıktısını okur; `lo` atlanır. Başlıklar ve bozuk satırlar yok sayılır;
/// hiçbir arayüz satırı tanınmazsa hata döner.
pub fn parse_net_dev(input: &str) -> Result<Vec<NetDev>, ParseError> {
    if input.trim().is_empty() {
        return Err(ParseError::Empty);
    }
    let mut recognized = 0usize;
    let mut out = Vec::new();
    for line in input.lines() {
        // Başlık satırlarında ':' yoktur.
        let Some((iface, stats)) = line.split_once(':') else {
            continue;
        };
        let iface = iface.trim();
        let f: Vec<&str> = stats.split_whitespace().collect();
        // rx bytes = 0. alan, tx bytes = 8. alan.
        let (Some(rx), Some(tx)) = (f.first(), f.get(8)) else {
            continue;
        };
        let (Ok(rx_bytes), Ok(tx_bytes)) = (rx.parse::<u64>(), tx.parse::<u64>()) else {
            continue;
        };
        if iface.is_empty() {
            continue;
        }
        recognized += 1;
        if iface == "lo" {
            continue;
        }
        out.push(NetDev {
            iface: iface.to_string(),
            rx_bytes,
            tx_bytes,
        });
    }
    if recognized == 0 {
        return Err(malformed("net/dev", input.lines().last().unwrap_or("")));
    }
    Ok(out)
}

/// İki örnek arasında arayüz başına bayt/sn hesaplar. Yalnızca iki örnekte de
/// bulunan arayüzler döner. Sayaç geri gittiyse (reset/wrap) o yön 0.0 olur;
/// `elapsed` sıfırsa tüm hızlar 0.0'dır.
pub fn net_rates(prev: &[NetDev], cur: &[NetDev], elapsed: Duration) -> Vec<NetRate> {
    let secs = elapsed.as_secs_f64();
    let rate = |p: u64, c: u64| {
        if secs <= 0.0 || c < p {
            0.0
        } else {
            (c - p) as f64 / secs
        }
    };
    cur.iter()
        .filter_map(|c| {
            let p = prev.iter().find(|p| p.iface == c.iface)?;
            Some(NetRate {
                iface: c.iface.clone(),
                rx_bps: rate(p.rx_bytes, c.rx_bytes),
                tx_bps: rate(p.tx_bytes, c.tx_bytes),
            })
        })
        .collect()
}

/// Tüm arayüzlerin toplam (rx, tx) hızı.
pub fn total_rate(rates: &[NetRate]) -> (f64, f64) {
    rates
        .iter()
        .fold((0.0, 0.0), |(rx, tx), r| (rx + r.rx_bps, tx + r.tx_bps))
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: &str = include_str!("../../tests/fixtures/net_dev.txt");
    const BAD: &str = include_str!("../../tests/fixtures/malformed_net_dev.txt");

    fn dev(i: &str, rx: u64, tx: u64) -> NetDev {
        NetDev {
            iface: i.into(),
            rx_bytes: rx,
            tx_bytes: tx,
        }
    }

    #[test]
    fn parses_fixture_skips_lo() {
        let d = parse_net_dev(N).unwrap();
        let names: Vec<_> = d.iter().map(|x| x.iface.as_str()).collect();
        assert_eq!(names, ["eth0", "docker0", "wg0"]);
        assert_eq!(d[0].rx_bytes, 9876543210);
        assert_eq!(d[0].tx_bytes, 1234567890);
        // "wg0:12345678" (boşluksuz) doğru okunur
        assert_eq!(d[2].rx_bytes, 12345678);
        assert_eq!(d[2].tx_bytes, 87654321);
    }

    #[test]
    fn empty_garbage_malformed() {
        assert_eq!(parse_net_dev(""), Err(ParseError::Empty));
        assert!(parse_net_dev("hello\nworld").is_err());
        assert!(parse_net_dev(BAD).is_err());
    }

    #[test]
    fn only_lo_is_ok_but_empty() {
        let line = "lo: 1 1 0 0 0 0 0 0 2 1 0 0 0 0 0 0\n";
        assert_eq!(parse_net_dev(line).unwrap(), vec![]);
    }

    #[test]
    fn huge_numbers() {
        let l = format!("eth0: {m} 0 0 0 0 0 0 0 {m} 0\n", m = u64::MAX);
        let d = parse_net_dev(&l).unwrap();
        assert_eq!(d[0].rx_bytes, u64::MAX);
        assert!(parse_net_dev("eth0: 99999999999999999999 0 0 0 0 0 0 0 1\n").is_err());
    }

    #[test]
    fn rates_basic() {
        let p = [dev("eth0", 1000, 2000)];
        let c = [dev("eth0", 3000, 2500)];
        let r = net_rates(&p, &c, Duration::from_secs(2));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].rx_bps, 1000.0);
        assert_eq!(r[0].tx_bps, 250.0);
        assert_eq!(total_rate(&r), (1000.0, 250.0));
    }

    #[test]
    fn rates_counter_reset_and_zero_elapsed() {
        let p = [dev("eth0", 5000, 5000)];
        let c = [dev("eth0", 100, 6000)];
        let r = net_rates(&p, &c, Duration::from_secs(1));
        assert_eq!(r[0].rx_bps, 0.0);
        assert_eq!(r[0].tx_bps, 1000.0);
        let r = net_rates(&p, &c, Duration::ZERO);
        assert_eq!((r[0].rx_bps, r[0].tx_bps), (0.0, 0.0));
    }

    #[test]
    fn rates_interface_set_changes() {
        let p = [dev("eth0", 0, 0), dev("gone", 0, 0)];
        let c = [dev("eth0", 10, 10), dev("new", 5, 5)];
        let r = net_rates(&p, &c, Duration::from_secs(1));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].iface, "eth0");
        assert!(net_rates(&[], &c, Duration::from_secs(1)).is_empty());
    }

    #[test]
    fn rates_huge_values_no_panic() {
        let p = [dev("e", 0, 0)];
        let c = [dev("e", u64::MAX, u64::MAX)];
        let r = net_rates(&p, &c, Duration::from_millis(1));
        assert!(r[0].rx_bps.is_finite());
    }
}
