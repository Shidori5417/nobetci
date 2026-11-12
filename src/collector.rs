//! Sunucu başına bir tokio task'ı: tek SSH oturumu açık tutulur, her tick'te
//! tek bir exec çalışır. UI'ye bağımlı değildir; sonuçlar `watch` kanalıyla akar.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::watch;

use crate::config::ServerConfig;
use crate::model::{CpuStat, NetDev, ServerSnapshot};
use crate::parser::{self, ParsedBatch, cpu::cpu_usage, net};
use crate::ssh;

/// Yeniden bağlanma bekleme süresi: 2 sn'den başlar, her hatada ikiye katlanır, 60 sn'de durur.
const BACKOFF_START: Duration = Duration::from_secs(2);
const BACKOFF_MAX: Duration = Duration::from_secs(60);

/// Exponential backoff durumu. Başarılı bir tick'ten sonra `reset` çağrılır.
struct Backoff(Duration);

impl Backoff {
    fn new() -> Self {
        Self(BACKOFF_START)
    }

    fn reset(&mut self) {
        self.0 = BACKOFF_START;
    }

    /// Şimdiki bekleme süresini verir ve bir sonrakini iki katına çıkarır.
    fn next_delay(&mut self) -> Duration {
        let d = self.0;
        self.0 = (self.0 * 2).min(BACKOFF_MAX);
        d
    }
}

/// Sunucuların aynı anda saldırmaması için süreye en çok %10 ekler.
// VARSAYIM: rand yok; sunucu indeksi ve saat nanosaniyesinden türetilir.
fn jitter(base: Duration, idx: usize) -> Duration {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.subsec_nanos() as u64);
    let span_ms = (base.as_millis() as u64 / 10).max(1);
    base + Duration::from_millis((nanos / 1000 + idx as u64 * 37) % span_ms)
}

/// Bir önceki örnekten kalan sayaçlar (fark hesabı için).
#[derive(Default)]
struct Prev {
    stat: Option<CpuStat>,
    net: Option<Vec<NetDev>>,
    at: Option<Instant>,
}

/// Ayrıştırılmış çıktıdan snapshot üretir. Bozuk bölüm yalnızca o metriği `None` yapar.
fn snapshot_from(name: &str, batch: ParsedBatch, prev: &mut Prev, now: Instant) -> ServerSnapshot {
    let elapsed = prev.at.map_or(Duration::ZERO, |t| now.duration_since(t));
    let cpu = match (&prev.stat, &batch.stat) {
        (Some(p), Ok(c)) => Some(cpu_usage(p, c)),
        _ => None,
    };
    let rates = match (&prev.net, &batch.net) {
        (Some(p), Ok(c)) => Some(net::total_rate(&net::net_rates(p, c, elapsed))),
        _ => None,
    };
    prev.stat = batch.stat.ok();
    prev.net = batch.net.ok();
    prev.at = Some(now);
    ServerSnapshot {
        name: name.to_string(),
        checked: true,
        reachable: true,
        cpu,
        mem: batch.mem.ok(),
        load: batch.load.ok(),
        disks: batch.disks.ok(),
        uptime: batch.uptime.ok(),
        services: batch.services.unwrap_or_default(),
        net_rx_bps: rates.map(|r| r.0),
        net_tx_bps: rates.map(|r| r.1),
        error: None,
        seq: 0,
        retry_at: None,
    }
}

fn unreachable_snapshot(name: &str, err: String, retry_at: Instant) -> ServerSnapshot {
    ServerSnapshot {
        name: name.to_string(),
        checked: true,
        reachable: false,
        error: Some(err),
        retry_at: Some(retry_at),
        ..Default::default()
    }
}

type Tx = Arc<watch::Sender<Vec<ServerSnapshot>>>;

async fn run_server(idx: usize, server: ServerConfig, interval: Duration, tx: Tx) {
    // Her yayında artan numara: UI aynı örneği geçmişe iki kez eklemez.
    let mut seq = 0u64;
    let mut publish = |mut s: ServerSnapshot| {
        seq += 1;
        s.seq = seq;
        tx.send_modify(|v| v[idx] = s);
    };
    let cmd = parser::build_command(&server.services);
    let mut backoff = Backoff::new();
    loop {
        let session = match ssh::resolve(&server) {
            Ok(t) => ssh::connect(&t).await,
            Err(e) => Err(e),
        };
        let err = match session {
            Ok(session) => {
                tracing::info!("{}: bağlandı", server.name);
                let mut prev = Prev::default();
                let mut tick = tokio::time::interval(interval);
                tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    tick.tick().await;
                    match session.exec(&cmd).await {
                        Ok(out) => {
                            backoff.reset();
                            publish(snapshot_from(
                                &server.name,
                                parser::parse_batch(&out),
                                &mut prev,
                                Instant::now(),
                            ));
                        }
                        Err(e) => {
                            tracing::warn!("{}: okuma hatası: {e}", server.name);
                            break e;
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("{}: bağlanılamadı: {e}", server.name);
                e
            }
        };
        let delay = jitter(backoff.next_delay(), idx);
        tracing::info!(
            "{}: {:.1} sn sonra yeniden denenecek",
            server.name,
            delay.as_secs_f64()
        );
        publish(unreachable_snapshot(
            &server.name,
            err.to_string(),
            Instant::now() + delay,
        ));
        tokio::time::sleep(delay).await;
    }
}

/// Her sunucu için bir task başlatır; hepsi tek bir `watch` kanalına yazar.
/// Task'lar runtime kapanınca sonlanır.
pub fn spawn(servers: &[ServerConfig], interval: Duration) -> watch::Receiver<Vec<ServerSnapshot>> {
    let initial = servers
        .iter()
        .map(|s| ServerSnapshot {
            name: s.name.clone(),
            ..Default::default()
        })
        .collect();
    let (tx, rx) = watch::channel(initial);
    let tx = Arc::new(tx);
    for (idx, s) in servers.iter().cloned().enumerate() {
        tokio::spawn(run_server(idx, s, interval, tx.clone()));
    }
    rx
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{cpu::parse_stat, net::parse_net_dev};

    const S1: &str = include_str!("../tests/fixtures/proc_stat.txt");
    const S2: &str = include_str!("../tests/fixtures/proc_stat_2.txt");
    const N: &str = include_str!("../tests/fixtures/net_dev.txt");
    const OUT: &str = include_str!("../tests/fixtures/batch_output.txt");

    #[test]
    fn ilk_ornekte_cpu_yok_ikincide_var() {
        let mut prev = Prev::default();
        let t0 = Instant::now();
        let mut b = parser::parse_batch(OUT);
        b.stat = parse_stat(S1);
        b.net = parse_net_dev(N);
        let s = snapshot_from("a", b.clone(), &mut prev, t0);
        assert!(s.checked && s.reachable && s.cpu.is_none() && s.net_rx_bps.is_none());
        b.stat = parse_stat(S2);
        let s = snapshot_from("a", b, &mut prev, t0 + Duration::from_secs(2));
        assert!(s.cpu.is_some() && s.net_rx_bps.is_some());
    }

    #[test]
    fn backoff_ikiye_katlanir_ve_sifirlanir() {
        let mut b = Backoff::new();
        let s: Vec<u64> = (0..7).map(|_| b.next_delay().as_secs()).collect();
        assert_eq!(s, [2, 4, 8, 16, 32, 60, 60]);
        b.reset();
        assert_eq!(b.next_delay(), BACKOFF_START);
    }

    #[test]
    fn jitter_en_cok_yuzde_on() {
        for i in 0..20 {
            let j = jitter(Duration::from_secs(10), i);
            assert!(j >= Duration::from_secs(10) && j < Duration::from_millis(11_000));
        }
    }

    #[test]
    fn bozuk_bolum_metrigi_bilinmez_yapar() {
        let mut prev = Prev::default();
        let b = parser::parse_batch("çöp");
        let s = snapshot_from("a", b, &mut prev, Instant::now());
        assert!(s.mem.is_none() && s.load.is_none() && s.disks.is_none() && s.uptime.is_none());
        assert!(s.reachable && s.error.is_none());
    }
}
