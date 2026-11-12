//! `/proc/stat` ayrıştırma ve iki örnek arası CPU kullanımı.

use super::{ParseError, malformed};
use crate::model::{CpuStat, CpuTimes, CpuUsage};

/// `/proc/stat` çıktısını okur: `cpu` toplam satırı ve `cpuN` çekirdek satırları.
/// Diğer satırlar (`intr`, `ctxt`, ...) yok sayılır. Çekirdekler numarasına göre sıralanır.
pub fn parse_stat(input: &str) -> Result<CpuStat, ParseError> {
    if input.trim().is_empty() {
        return Err(ParseError::Empty);
    }
    let mut total = None;
    let mut cores: Vec<(usize, CpuTimes)> = Vec::new();

    for line in input.lines() {
        let mut it = line.split_whitespace();
        let Some(label) = it.next() else { continue };
        let Some(suffix) = label.strip_prefix("cpu") else {
            continue;
        };
        let is_total = suffix.is_empty();
        if !is_total && !suffix.bytes().all(|b| b.is_ascii_digit()) {
            // "cpufreq" gibi başka önekler sessizce atlanır.
            continue;
        }
        let times = parse_times(it, line)?;
        if is_total {
            total = Some(times);
        } else {
            let idx: usize = suffix.parse().map_err(|_| malformed("cpu", line))?;
            cores.push((idx, times));
        }
    }

    let total = total.ok_or(ParseError::MissingField("cpu"))?;
    cores.sort_by_key(|(i, _)| *i);
    Ok(CpuStat {
        total,
        cores: cores.into_iter().map(|(_, t)| t).collect(),
    })
}

fn parse_times<'a>(
    fields: impl Iterator<Item = &'a str>,
    line: &str,
) -> Result<CpuTimes, ParseError> {
    // guest/guest_nice user/nice içinde zaten sayıldığı için ilk 8 alan yeterli.
    let mut v = [0u64; 8];
    let mut n = 0;
    for (slot, f) in v.iter_mut().zip(fields) {
        *slot = f.parse().map_err(|_| malformed("cpu", line))?;
        n += 1;
    }
    // VARSAYIM: en az user/nice/system/idle gerekir; eski çekirdeklerde
    // iowait ve sonrası eksik olabilir, 0 kabul edilir.
    if n < 4 {
        return Err(malformed("cpu", line));
    }
    Ok(CpuTimes {
        user: v[0],
        nice: v[1],
        system: v[2],
        idle: v[3],
        iowait: v[4],
        irq: v[5],
        softirq: v[6],
        steal: v[7],
    })
}

fn sum(t: &CpuTimes) -> u128 {
    [
        t.user, t.nice, t.system, t.idle, t.iowait, t.irq, t.softirq, t.steal,
    ]
    .iter()
    .map(|&x| u128::from(x))
    .sum()
}

fn busy(t: &CpuTimes) -> u128 {
    // idle ve iowait "boş" sayılır.
    sum(t).saturating_sub(u128::from(t.idle) + u128::from(t.iowait))
}

/// İki örnek arası meşgul yüzdesi. Toplam sayaç ilerlemediyse veya geri gittiyse
/// (sıfırlanma/wrap) 0.0 döner.
fn percent(prev: &CpuTimes, cur: &CpuTimes) -> f64 {
    let (pt, ct) = (sum(prev), sum(cur));
    if ct <= pt {
        return 0.0;
    }
    let dt = (ct - pt) as f64;
    let db = busy(cur).saturating_sub(busy(prev)) as f64;
    (db / dt * 100.0).clamp(0.0, 100.0)
}

/// İki `/proc/stat` örneğinden CPU kullanımı (%) hesaplar.
/// Çekirdek sayısı değiştiyse çekirdekler indekse göre eşlenir; önceki örneği
/// olmayan çekirdek 0.0 olur (VARSAYIM: hot-plug nadir, tahmin yürütmüyoruz).
pub fn cpu_usage(prev: &CpuStat, cur: &CpuStat) -> CpuUsage {
    CpuUsage {
        total: percent(&prev.total, &cur.total),
        cores: cur
            .cores
            .iter()
            .enumerate()
            .map(|(i, c)| prev.cores.get(i).map_or(0.0, |p| percent(p, c)))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S1: &str = include_str!("../../tests/fixtures/proc_stat.txt");
    const S2: &str = include_str!("../../tests/fixtures/proc_stat_2.txt");
    const BAD: &str = include_str!("../../tests/fixtures/malformed_stat.txt");

    #[test]
    fn parses_fixture() {
        let s = parse_stat(S1).unwrap();
        assert_eq!(s.cores.len(), 4);
        assert_eq!(s.cores[0].user, 1393280);
        assert_eq!(s.cores[0].idle, 13343292);
        assert_eq!(s.cores[0].softirq, 17875);
        assert_eq!(s.total.iowait, 16683);
    }

    #[test]
    fn usage_from_fixtures() {
        let u = cpu_usage(&parse_stat(S1).unwrap(), &parse_stat(S2).unwrap());
        assert!((u.total - 20.0).abs() < 1e-9, "{}", u.total);
        let want = [50.0, 10.0, 0.0, 20.0];
        assert_eq!(u.cores.len(), 4);
        for (g, w) in u.cores.iter().zip(want) {
            assert!((g - w).abs() < 1e-9, "{g} != {w}");
        }
    }

    #[test]
    fn empty_and_garbage() {
        assert_eq!(parse_stat(""), Err(ParseError::Empty));
        assert_eq!(parse_stat("  \n\n"), Err(ParseError::Empty));
        assert_eq!(
            parse_stat("hello\nworld 1 2 3\n"),
            Err(ParseError::MissingField("cpu"))
        );
        assert!(parse_stat(BAD).is_err());
    }

    #[test]
    fn truncated_line() {
        assert!(parse_stat("cpu  1 2 3\n").is_err());
        assert!(parse_stat("cpu  1 2 3 4\ncpu0 1 2").is_err());
        assert!(parse_stat("cpu  1 2 3 4\ncpu0 1 2 3 4").is_ok());
    }

    #[test]
    fn old_kernel_four_fields() {
        let s = parse_stat("cpu  10 0 5 85\n").unwrap();
        assert_eq!(s.total.idle, 85);
        assert_eq!(s.total.iowait, 0);
        assert!(s.cores.is_empty());
    }

    #[test]
    fn huge_and_overflowing_numbers() {
        let max = u64::MAX;
        let line = format!("cpu  {max} {max} {max} {max} {max} {max} {max} {max}\n");
        let s = parse_stat(&line).unwrap();
        assert_eq!(s.total.user, max);
        // sayı u64 dışı: hata, panik yok
        assert!(parse_stat("cpu  99999999999999999999 0 0 0\n").is_err());
        // u128 toplamı taşmaz
        let u = cpu_usage(&CpuStat::default(), &s);
        assert!((u.total - 75.0).abs() < 1e-9);
    }

    #[test]
    fn zero_delta_is_zero() {
        let s = parse_stat(S1).unwrap();
        let u = cpu_usage(&s, &s);
        assert_eq!(u.total, 0.0);
        assert!(u.cores.iter().all(|&c| c == 0.0));
    }

    #[test]
    fn counter_reset_is_zero() {
        let a = parse_stat(S2).unwrap();
        let b = parse_stat(S1).unwrap();
        assert_eq!(cpu_usage(&a, &b).total, 0.0);
    }

    #[test]
    fn core_count_change() {
        let full = parse_stat(S1).unwrap();
        let two = CpuStat {
            total: full.total,
            cores: full.cores[..2].to_vec(),
        };
        let later = parse_stat(S2).unwrap();
        // önceki 2, şimdiki 4 çekirdek
        let u = cpu_usage(&two, &later);
        assert_eq!(u.cores.len(), 4);
        assert_eq!(u.cores[2], 0.0);
        assert_eq!(u.cores[3], 0.0);
        // önceki 4, şimdiki 2 çekirdek
        let u = cpu_usage(
            &full,
            &CpuStat {
                cores: later.cores[..2].to_vec(),
                ..later
            },
        );
        assert_eq!(u.cores.len(), 2);
    }

    #[test]
    fn cores_sorted_and_unknown_prefix_ignored() {
        let s = parse_stat("cpu 1 1 1 1\ncpu1 2 2 2 2\ncpu0 3 3 3 3\ncpufreq x\n").unwrap();
        assert_eq!(s.cores[0].user, 3);
        assert_eq!(s.cores[1].user, 2);
    }

    #[test]
    fn busy_excludes_iowait() {
        let a = CpuStat::default();
        let b = CpuStat {
            total: CpuTimes {
                user: 10,
                idle: 80,
                iowait: 10,
                ..Default::default()
            },
            cores: vec![],
        };
        assert!((cpu_usage(&a, &b).total - 10.0).abs() < 1e-9);
    }
}
