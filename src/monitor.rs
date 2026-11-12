//! TUI ve daemon'un ortak çekirdeği: collector'lar + uyarı motoru.
//!
//! Collector'lar snapshot'ları `watch` ile yayar; motor görevi bunları (ve 1 sn'lik
//! bir tick'i, `for` süresi dolsun diye) değerlendirir. Durum değişimleri `events`
//! kanalına, anlık durum (aktif + geçmiş) `alerts` kanalına gider.

use std::time::{Duration, Instant, SystemTime};

use tokio::sync::{mpsc, watch};

use crate::alert::{ActiveAlert, AlertEngine, AlertEvent, AlertRule};
use crate::config::{self, Config};
use crate::model::ServerSnapshot;
use crate::{collector, demo};

/// Uyarı ekranının okuduğu anlık durum.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AlertState {
    pub active: Vec<ActiveAlert>,
    /// Yeniden eskiye.
    pub history: Vec<AlertEvent>,
}

pub struct Monitor {
    pub snapshots: watch::Receiver<Vec<ServerSnapshot>>,
    pub alerts: watch::Receiver<AlertState>,
    /// Firing/Resolved olayları. Okunmayacaksa düşürülmeli (kuyruk birikmesin).
    pub events: mpsc::UnboundedReceiver<AlertEvent>,
}

/// Gerçek sunucular için collector'ları ve motoru başlatır.
pub fn from_config(cfg: &Config) -> Monitor {
    start(
        collector::spawn(&cfg.servers, cfg.interval()),
        rules(&cfg.alerts),
    )
}

/// Sahte veri + demo kuralları.
pub fn demo() -> Monitor {
    start(demo::spawn(), demo::alert_rules())
}

// VARSAYIM: kurallar `Config::parse` içinde doğrulanmıştır; ayrıştırılamayan atlanır.
fn rules(cfg: &[config::AlertRule]) -> Vec<AlertRule> {
    cfg.iter()
        .filter_map(|r| AlertRule::from_config(r).ok())
        .collect()
}

pub fn start(snapshots: watch::Receiver<Vec<ServerSnapshot>>, rules: Vec<AlertRule>) -> Monitor {
    let (state_tx, alerts) = watch::channel(AlertState::default());
    let (ev_tx, events) = mpsc::unbounded_channel();
    let mut rx = snapshots.clone();
    tokio::spawn(async move {
        let mut engine = AlertEngine::new(rules);
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                r = rx.changed() => if r.is_err() { break },
                _ = tick.tick() => {}
            }
            let snap = rx.borrow_and_update().clone();
            let evs = engine.evaluate(Instant::now(), SystemTime::now(), &snap);
            if evs.is_empty() {
                continue;
            }
            state_tx.send_replace(AlertState {
                active: engine.active(),
                history: engine.history().iter().rev().cloned().collect(),
            });
            for e in evs {
                let _ = ev_tx.send(e); // alıcı yoksa (TUI) olay atılır
            }
        }
    });
    Monitor {
        snapshots,
        alerts,
        events,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::alert::AlertEventKind;

    #[tokio::test]
    async fn motor_olay_ve_durum_yayinlar() {
        let snap = |reachable| {
            vec![ServerSnapshot {
                name: "a".into(),
                checked: true,
                reachable,
                ..Default::default()
            }]
        };
        let rule = AlertRule::parse("unreachable", None, vec![]).unwrap();
        let (tx, rx) = watch::channel(snap(true));
        let mut m = start(rx, vec![rule]);
        tx.send_replace(snap(false));
        let e = m.events.recv().await.unwrap();
        assert_eq!(e.kind, AlertEventKind::Firing);
        assert_eq!(m.alerts.borrow().active.len(), 1);
        tx.send_replace(snap(true));
        let e = m.events.recv().await.unwrap();
        assert_eq!(e.kind, AlertEventKind::Resolved);
        let st = m.alerts.borrow();
        assert!(st.active.is_empty());
        assert_eq!(st.history.len(), 2);
        assert_eq!(st.history[0].kind, AlertEventKind::Resolved); // yeniden eskiye
    }

    #[test]
    fn demo_kurallari_gecerli() {
        assert_eq!(demo::alert_rules().len(), 2);
    }
}
