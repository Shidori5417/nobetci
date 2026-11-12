//! Yazma işlemleri (servis restart) ve log akışı başlatma. `allow_actions = false`
//! olan sunucuda hiçbir işlem çalışmaz; kontrol hem UI'de hem burada yapılır.
//!
//! VARSAYIM: Her işlem için kısa ömürlü, ayrı bir SSH oturumu açılır. Collector'ın
//! oturumuna komut kanalı eklemek izleme döngüsünü karmaşıklaştırırdı; restart
//! nadir bir işlem olduğundan bağlantı maliyeti kabul edilebilir.

use std::time::Duration;

use tokio::sync::mpsc;

use crate::config::ServerConfig;
use crate::logs::{self, LogMsg};
use crate::parser::valid_service_name;
use crate::ssh::{self, ExecOutput};

/// Bir işlemin sonucu. `result`: Ok(özet) ya da Err(kullanıcıya gösterilecek hata).
#[derive(Debug, Clone)]
pub struct Outcome {
    pub server: String,
    pub service: String,
    pub result: Result<String, String>,
}

pub struct Actions {
    servers: Vec<ServerConfig>,
    demo: bool,
    tx: mpsc::UnboundedSender<Outcome>,
}

impl Actions {
    /// `servers` sırası, UI'deki sunucu sırasıyla aynı olmalıdır.
    pub fn new(servers: Vec<ServerConfig>, demo: bool) -> (Self, mpsc::UnboundedReceiver<Outcome>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (Self { servers, demo, tx }, rx)
    }

    pub fn allowed(&self, idx: usize) -> bool {
        self.servers.get(idx).is_some_and(|s| s.allow_actions)
    }

    /// Servisi arka planda yeniden başlatır; sonuç `Outcome` olarak kanala düşer.
    pub fn restart(&self, idx: usize, service: String) {
        let Some(server) = self.servers.get(idx).cloned() else {
            return;
        };
        let (tx, demo) = (self.tx.clone(), self.demo);
        tokio::spawn(async move {
            let result = if !server.allow_actions {
                Err("Bu sunucuda işlemler kapalı (allow_actions = false)".to_string())
            } else if demo {
                tokio::time::sleep(Duration::from_millis(1200)).await;
                Ok(format!("{service} yeniden başlatıldı (demo)"))
            } else {
                restart(&server, &service).await
            };
            tracing::info!(
                "{}: restart {service}: {}",
                server.name,
                match &result {
                    Ok(_) => "başarılı".to_string(),
                    Err(e) => format!("başarısız: {e}"),
                }
            );
            let _ = tx.send(Outcome {
                server: server.name,
                service,
                result,
            });
        });
    }

    /// Log akışı başlatır (`unit`: tek servis ya da `None` = tüm günlük).
    /// Dönen alıcı düşürülünce uzak komut durur.
    pub fn logs(&self, idx: usize, unit: Option<String>) -> Option<mpsc::Receiver<LogMsg>> {
        let server = self.servers.get(idx)?.clone();
        Some(if self.demo {
            logs::spawn_demo(server.name, unit)
        } else {
            logs::spawn(server, unit)
        })
    }
}

/// Uzakta çalışacak restart komutu. Root mu karar uzakta, çalışma anında verilir:
/// root ise doğrudan, değilse `sudo -n` (parola istemez, gerekiyorsa hata verir).
/// Ad, parser'daki izin listesiyle doğrulanır; betik tek tırnak içindedir.
// VARSAYIM: `sh` ve `id` her hedefte vardır; kullanıcının giriş kabuğu ne olursa olsun `sh -c` çalışır.
pub fn restart_command(service: &str) -> Result<String, String> {
    if !valid_service_name(service) {
        return Err(format!("geçersiz servis adı: {service:?}"));
    }
    Ok(format!(
        "sh -c 'if [ \"$(id -u)\" = 0 ]; then exec systemctl restart -- {service}; \
         else exec sudo -n systemctl restart -- {service}; fi'"
    ))
}

/// Çıkış kodu ve stderr'den kullanıcıya gösterilecek tek satırlık sonuç.
fn describe(service: &str, out: &ExecOutput) -> Result<String, String> {
    if out.status == Some(0) {
        return Ok(format!("{service} yeniden başlatıldı"));
    }
    // stderr güvenilmezdir: temizlenir, tek satıra indirilir ve kısaltılır.
    let err: String = out
        .stderr
        .lines()
        .map(logs::sanitize_line)
        .filter(|l| !l.trim().is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(200)
        .collect();
    let code = out.status.map_or("sinyalle sonlandı".to_string(), |c| {
        format!("çıkış kodu {c}")
    });
    Err(if err.is_empty() {
        format!("{service}: başarısız ({code})")
    } else {
        format!("{service}: {err} ({code})")
    })
}

async fn restart(server: &ServerConfig, service: &str) -> Result<String, String> {
    let target = ssh::resolve(server).map_err(|e| e.to_string())?;
    let cmd = restart_command(service)?;
    let session = ssh::connect(&target)
        .await
        .map_err(|e| format!("bağlanılamadı: {e}"))?;
    let out = session.run(&cmd).await;
    session.disconnect().await;
    describe(service, &out.map_err(|e| e.to_string())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn komut_calisma_aninda_root_kontrol_eder() {
        let c = restart_command("nginx").unwrap();
        assert_eq!(
            c,
            "sh -c 'if [ \"$(id -u)\" = 0 ]; then exec systemctl restart -- nginx; \
             else exec sudo -n systemctl restart -- nginx; fi'"
        );
        // Betik tek tırnak içinde; içinde başka tek tırnak olmamalı.
        assert_eq!(c.matches('\'').count(), 2);
        for bad in ["", "-x", "a b", "a;b", "$(x)", "a'b"] {
            assert!(restart_command(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn sonuc_ozeti_stderr_temizler_ve_kisaltir() {
        let ok = ExecOutput {
            status: Some(0),
            ..Default::default()
        };
        assert!(describe("x", &ok).is_ok());
        let bad = ExecOutput {
            stderr: "sudo: a password is required\n\x1b[31mikinci\x1b[0m\n".into(),
            status: Some(1),
            ..Default::default()
        };
        assert_eq!(
            describe("x", &bad).unwrap_err(),
            "x: sudo: a password is required ikinci (çıkış kodu 1)"
        );
        let long = ExecOutput {
            stderr: "a".repeat(1000),
            status: None,
            ..Default::default()
        };
        assert!(describe("x", &long).unwrap_err().len() < 260);
    }

    #[tokio::test]
    async fn kapali_sunucuda_islem_calismaz() {
        let s = |allow| ServerConfig {
            name: "s".into(),
            ssh_host: None,
            host: Some("127.0.0.1".into()),
            user: Some("u".into()),
            port: Some(1),
            key: None,
            services: vec![],
            allow_actions: allow,
        };
        let (a, mut rx) = Actions::new(vec![s(false), s(true)], true);
        assert!(!a.allowed(0) && a.allowed(1) && !a.allowed(9));
        a.restart(0, "nginx".into());
        let o = rx.recv().await.unwrap();
        assert!(o.result.unwrap_err().contains("allow_actions = false"));
        a.restart(1, "nginx".into());
        assert!(rx.recv().await.unwrap().result.is_ok());
    }
}
