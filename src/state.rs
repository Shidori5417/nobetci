//! Sunucu başına son durum + ring buffer geçmişi (sparkline için).
//!
//! VARSAYIM: Geçmiş UI tarafında birikir. Collector yalnızca son snapshot'ı
//! `watch` kanalına yazar; UI her yeni `ServerSnapshot::seq` değerinde örneği
//! geçmişe ekler. Böylece collector/daemon modu geçmişi tutmak zorunda kalmaz,
//! aynı örnek (ör. yalnızca yeniden çizim) iki kez eklenmez. Bedeli: UI
//! kapalıyken geçmiş birikmez.

use std::collections::VecDeque;

use crate::model::ServerSnapshot;

/// Her seride tutulan en fazla örnek sayısı.
pub const CAPACITY: usize = 120;

/// Sabit kapasiteli seriye ekler; dolunca en eskiyi atar.
pub fn push_capped(buf: &mut VecDeque<f64>, v: f64) {
    if buf.len() == CAPACITY {
        buf.pop_front();
    }
    buf.push_back(v);
}

/// Bir sunucunun son snapshot'ı ve geçmişi.
#[derive(Debug, Clone, Default)]
pub struct ServerState {
    pub latest: ServerSnapshot,
    last_seq: u64,
    /// Toplam CPU %.
    pub cpu: VecDeque<f64>,
    /// Çekirdek başına CPU %.
    pub cores: Vec<VecDeque<f64>>,
    /// RAM kullanımı %.
    pub ram: VecDeque<f64>,
    /// Ağ hızları, bayt/sn.
    pub net_rx: VecDeque<f64>,
    pub net_tx: VecDeque<f64>,
}

impl ServerState {
    /// Snapshot'ı alır. Yeni bir örnekse (seq değiştiyse) geçmişe ekler ve `true` döner.
    /// `seq == 0` henüz hiç yayın yapılmadığı anlamına gelir; geçmişe yazılmaz.
    pub fn push(&mut self, snap: &ServerSnapshot) -> bool {
        let fresh = snap.seq != 0 && snap.seq != self.last_seq;
        self.latest = snap.clone();
        if !fresh {
            return false;
        }
        self.last_seq = snap.seq;
        // Erişilemez örnekler geçmişte boşluk bırakmaz, hiç eklenmez.
        if !snap.reachable {
            return true;
        }
        if let Some(cpu) = &snap.cpu {
            push_capped(&mut self.cpu, cpu.total);
            self.cores.resize_with(cpu.cores.len(), VecDeque::new);
            for (buf, v) in self.cores.iter_mut().zip(&cpu.cores) {
                push_capped(buf, *v);
            }
        }
        if let Some(m) = snap.mem.filter(|m| m.total > 0) {
            let used = m.total - m.available.min(m.total);
            push_capped(&mut self.ram, 100.0 * used as f64 / m.total as f64);
        }
        if let Some(v) = snap.net_rx_bps {
            push_capped(&mut self.net_rx, v);
        }
        if let Some(v) = snap.net_tx_bps {
            push_capped(&mut self.net_tx, v);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CpuUsage, MemInfo};

    fn snap(seq: u64, cpu: f64) -> ServerSnapshot {
        ServerSnapshot {
            checked: true,
            reachable: true,
            seq,
            cpu: Some(CpuUsage {
                total: cpu,
                cores: vec![cpu, cpu / 2.0],
            }),
            mem: Some(MemInfo {
                total: 100,
                available: 75,
                ..Default::default()
            }),
            net_rx_bps: Some(10.0),
            net_tx_bps: Some(20.0),
            ..Default::default()
        }
    }

    #[test]
    fn kapasite_asilinca_eskiler_atilir_sira_korunur() {
        let mut st = ServerState::default();
        for i in 1..=(CAPACITY as u64 + 5) {
            assert!(st.push(&snap(i, i as f64)));
        }
        assert_eq!(st.cpu.len(), CAPACITY);
        assert_eq!(st.cpu.front().copied(), Some(6.0));
        assert_eq!(st.cpu.back().copied(), Some(CAPACITY as f64 + 5.0));
        assert_eq!(st.cores.len(), 2);
        assert_eq!(st.cores[1].len(), CAPACITY);
        assert_eq!(st.ram.back().copied(), Some(25.0));
        assert_eq!(st.net_tx.len(), CAPACITY);
    }

    #[test]
    fn ayni_seq_tekrar_eklenmez() {
        let mut st = ServerState::default();
        assert!(st.push(&snap(1, 10.0)));
        assert!(!st.push(&snap(1, 99.0)));
        assert_eq!(st.cpu.len(), 1);
        assert_eq!(st.latest.cpu.as_ref().map(|c| c.total), Some(99.0));
    }

    #[test]
    fn seq_sifir_ve_erisilemez_ornek_gecmise_girmez() {
        let mut st = ServerState::default();
        assert!(!st.push(&snap(0, 10.0)));
        let down = ServerSnapshot {
            seq: 1,
            ..Default::default()
        };
        assert!(st.push(&down));
        assert!(st.cpu.is_empty() && st.ram.is_empty());
    }
}
