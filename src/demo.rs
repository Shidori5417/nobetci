//! `--demo` için sahte ama gerçekçi, yavaş değişen veri.

use std::time::Duration;

use tokio::sync::watch;

use crate::model::{CpuUsage, DiskUsage, LoadAvg, MemInfo, ServerSnapshot, ServiceStatus};

const GIB: u64 = 1 << 30;

/// Deterministik sözde rastgele: (0.0..1.0), seed ve tick'e bağlı yumuşak dalga.
// VARSAYIM: rand crate'i gereksiz; sin tabanlı dalga yavaş değişim için yeterli.
fn wave(seed: f64, tick: u64, period: f64) -> f64 {
    let t = tick as f64 / period;
    let v = (t + seed).sin() * 0.6 + (t * 2.3 + seed * 1.7).sin() * 0.4;
    (v + 1.0) / 2.0
}

fn service(name: &str, up: bool) -> ServiceStatus {
    ServiceStatus {
        name: name.into(),
        load_state: "loaded".into(),
        active_state: if up { "active" } else { "failed" }.into(),
        sub_state: if up { "running" } else { "failed" }.into(),
    }
}

fn healthy(name: &str, seed: f64, tick: u64, cores: usize, disk_pct: f64) -> ServerSnapshot {
    let cpu_total = 10.0 + 40.0 * wave(seed, tick, 9.0);
    let mem_total = 8 * GIB;
    let mem_used = (mem_total as f64 * (0.35 + 0.3 * wave(seed + 1.0, tick, 25.0))) as u64;
    let disk_total = 80 * GIB;
    let disk_used = (disk_total as f64 * disk_pct / 100.0) as u64;
    ServerSnapshot {
        name: name.into(),
        checked: true,
        reachable: true,
        cpu: Some(CpuUsage {
            total: cpu_total,
            cores: (0..cores)
                .map(|i| (cpu_total + 15.0 * (wave(seed + i as f64, tick, 5.0) - 0.5)).max(0.0))
                .collect(),
        }),
        mem: Some(MemInfo {
            total: mem_total,
            available: mem_total - mem_used,
            swap_total: 2 * GIB,
            swap_free: 2 * GIB,
        }),
        load: Some(LoadAvg {
            one: cpu_total / 100.0 * cores as f64,
            five: 0.5 + wave(seed, tick, 40.0),
            fifteen: 0.4 + wave(seed, tick, 90.0) * 0.5,
        }),
        disks: Some(vec![DiskUsage {
            filesystem: "/dev/vda1".into(),
            fstype: "ext4".into(),
            mount: "/".into(),
            total: disk_total,
            used: disk_used,
            available: disk_total - disk_used,
        }]),
        uptime: Some(Duration::from_secs(
            (seed * 1000.0) as u64 * 600 + 86_400 * 12 + tick,
        )),
        services: vec![service("nginx", true), service("postgresql", true)],
        net_rx_bps: Some(2e5 + 1.5e6 * wave(seed + 2.0, tick, 11.0)),
        net_tx_bps: Some(5e4 + 4e5 * wave(seed + 3.0, tick, 7.0)),
        ..Default::default()
    }
}

/// Verilen saniye sayacı için sahte sunucu listesi üretir.
pub fn snapshots(tick: u64) -> Vec<ServerSnapshot> {
    let mut web = healthy("vds-1", 0.3, tick, 4, 41.0 + 3.0 * wave(0.3, tick, 200.0));
    web.services.push(service("minecraft", true));

    let mut db = healthy("db-1", 1.9, tick, 2, 93.0 + 2.0 * wave(1.9, tick, 120.0));
    db.services = vec![service("postgresql", true)];
    // Hiç yüklenmemiş unit: çökme değil, "bulunamadı" olarak görünür.
    db.services.push(ServiceStatus {
        name: "redis".into(),
        load_state: "not-found".into(),
        active_state: "inactive".into(),
        sub_state: "dead".into(),
    });

    let mut yedek = healthy("yedek", 4.2, tick, 2, 58.0);
    yedek.services = vec![service("nginx", true), service("minecraft", false)];

    let down = ServerSnapshot {
        name: "eski-vds".into(),
        checked: true,
        reachable: false,
        error: Some("bağlantı zaman aşımı (demo)".into()),
        ..Default::default()
    };

    let mut all = vec![web, db, yedek, down];
    for s in &mut all {
        s.seq = tick + 1;
    }
    all
}

/// Her saniye yeni veri gönderen arka plan görevi. UI yalnızca son değeri okur.
pub fn spawn() -> watch::Receiver<Vec<ServerSnapshot>> {
    let (tx, rx) = watch::channel(snapshots(0));
    tokio::spawn(async move {
        let mut tick = 0u64;
        let mut iv = tokio::time::interval(Duration::from_secs(1));
        loop {
            iv.tick().await;
            tick += 1;
            if tx.send(snapshots(tick)).is_err() {
                break; // alıcı kapandı
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministik_ve_degisken() {
        let a = snapshots(5);
        let b = snapshots(5);
        assert_eq!(a[0].cpu, b[0].cpu);
        assert_ne!(a[0].cpu, snapshots(6)[0].cpu);
        assert_eq!(a.len(), 4);
        assert!(!a[3].reachable);
    }
}
