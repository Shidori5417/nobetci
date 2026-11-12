//! TUI'siz çalışma: uyarı olaylarını bildirimlere çevirip gönderir.
//!
//! Gönderim ayrı task'larda yapılır; HTTP yavaşlığı değerlendirmeyi bloklamaz.

use std::sync::Arc;
use std::time::Duration;

use color_eyre::eyre::Result;
use tokio::signal::unix::{SignalKind, signal};
use tokio::task::JoinSet;

use crate::alert::{AlertEvent, AlertEventKind};
use crate::monitor::Monitor;
use crate::notify::{self, Notification, NotificationKind, Notifier};
use crate::timefmt;

/// Kapanışta uçuştaki gönderimler için tanınan en uzun süre.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Kuraldan okunur başlık parçası: "disk kullanımı uyarısı", "servis nginx çalışmıyor"...
fn label(ev: &AlertEvent) -> String {
    match (ev.rule.as_str(), &ev.subject) {
        ("unreachable", _) => "sunucuya ulaşılamıyor".into(),
        ("service_down", Some(s)) => format!("servis {s} çalışmıyor"),
        (r, _) => {
            let name = match r.split(['<', '>', ' ']).next().unwrap_or(r) {
                "cpu" => "CPU kullanımı",
                "mem" => "RAM kullanımı",
                "disk_usage" => "disk kullanımı",
                "swap" => "swap kullanımı",
                "load" => "load",
                _ => r,
            };
            format!("{name} uyarısı")
        }
    }
}

pub fn notification(ev: &AlertEvent) -> Notification {
    let (kind, title, body) = match ev.kind {
        AlertEventKind::Firing => (
            NotificationKind::Firing,
            format!("🔴 {}: {}", ev.server, label(ev)),
            format!(
                "{}\nKural: {}\nBaşlangıç: {}",
                ev.message,
                ev.rule,
                timefmt::local(ev.started_at)
            ),
        ),
        AlertEventKind::Resolved => (
            NotificationKind::Resolved,
            format!("✅ Çözüldü: {}: {}", ev.server, label(ev)),
            format!(
                "{}\nBaşlangıç: {}\nBitiş: {}",
                ev.message,
                timefmt::local(ev.started_at),
                timefmt::local(ev.at)
            ),
        ),
    };
    Notification {
        kind,
        server: ev.server.clone(),
        title,
        body,
        at: ev.at,
    }
}

/// SIGINT/SIGTERM gelene kadar olayları bildirime çevirir.
pub async fn run(mut monitor: Monitor, notifiers: Vec<Arc<dyn Notifier>>) -> Result<()> {
    let names: Vec<_> = notifiers.iter().map(|n| n.name()).collect();
    if notifiers.is_empty() {
        tracing::warn!("hiç bildirici yapılandırılmamış: uyarılar yalnızca loglanacak");
    }
    tracing::info!(bildiriciler = ?names, "daemon başladı");
    let mut term = signal(SignalKind::terminate())?;
    let mut sends = JoinSet::new();
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => { tracing::info!("SIGINT alındı, kapanılıyor"); break }
            _ = term.recv() => { tracing::info!("SIGTERM alındı, kapanılıyor"); break }
            ev = monitor.events.recv() => {
                let Some(ev) = ev else { break };
                tracing::info!(sunucu = %ev.server, kural = %ev.rule, durum = ?ev.kind, "{}", ev.message);
                let n = notification(&ev);
                let nf = notifiers.clone();
                sends.spawn(async move { notify::send_all(&nf, &n).await });
                while sends.try_join_next().is_some() {}
            }
        }
    }
    if tokio::time::timeout(DRAIN_TIMEOUT, async {
        while sends.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        tracing::warn!("bazı bildirimler kapanışta gönderilemedi");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::SystemTime;

    fn ev(kind: AlertEventKind, rule: &str, subject: Option<&str>) -> AlertEvent {
        AlertEvent {
            kind,
            key: "k".into(),
            server: "vds-1".into(),
            rule: rule.into(),
            subject: subject.map(Into::into),
            message: "mesaj".into(),
            started_at: SystemTime::now(),
            at: SystemTime::now(),
        }
    }

    #[test]
    fn baslik_ve_govde() {
        let n = notification(&ev(AlertEventKind::Firing, "disk_usage > 90", None));
        assert_eq!(n.title, "🔴 vds-1: disk kullanımı uyarısı");
        assert_eq!(n.kind, NotificationKind::Firing);
        assert!(
            n.body
                .starts_with("mesaj\nKural: disk_usage > 90\nBaşlangıç: ")
        );
        let n = notification(&ev(AlertEventKind::Resolved, "service_down", Some("nginx")));
        assert_eq!(n.title, "✅ Çözüldü: vds-1: servis nginx çalışmıyor");
        assert_eq!(n.kind, NotificationKind::Resolved);
        assert!(n.body.contains("Bitiş: "));
        let n = notification(&ev(AlertEventKind::Firing, "unreachable", None));
        assert!(n.title.ends_with("sunucuya ulaşılamıyor"));
    }
}
