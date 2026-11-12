//! Bildirim katmanı: Discord ve Telegram. Alert motorundan bağımsızdır;
//! entegrasyon alert olaylarını `Notification`'a çevirir.
//!
//! GÜVENLİK: webhook/bot token'ı URL içinde taşınır. Hiçbir hata veya log
//! satırına URL ya da token yazılmaz (`reqwest::Error::without_url`).

mod discord;
mod telegram;

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::config::NotifyConfig;

pub use discord::DiscordNotifier;
pub use telegram::TelegramNotifier;

/// Toplam deneme sayısı (ilk istek dahil).
const MAX_ATTEMPTS: u32 = 3;
/// İstek başına toplam zaman aşımı.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// VARSAYIM: 429 yanıtındaki bekleme süresi en fazla bu kadar dikkate alınır.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(30);
/// VARSAYIM: her bildirici için gönderimler arası en az 1 sn.
const MIN_INTERVAL: Duration = Duration::from_secs(1);
/// İlk yeniden deneme beklemesi; her denemede ikiye katlanır (1s, 2s).
const BACKOFF_BASE: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Firing,
    Resolved,
}

#[derive(Debug, Clone)]
pub struct Notification {
    pub kind: NotificationKind,
    pub server: String,
    pub title: String,
    pub body: String,
    pub at: SystemTime,
}

#[derive(Debug, thiserror::Error)]
pub enum NotifyError {
    /// URL'si temizlenmiş istek hatası (ağ, zaman aşımı, TLS).
    #[error("istek hatası: {0}")]
    Request(reqwest::Error),
    #[error("sunucu {0} durum kodu döndürdü")]
    Status(u16),
}

#[async_trait]
pub trait Notifier: Send + Sync {
    fn name(&self) -> &'static str;
    async fn send(&self, n: &Notification) -> Result<(), NotifyError>;
}

/// Config'te tanımlı bildiricileri kurar.
pub fn from_config(cfg: &NotifyConfig) -> Vec<Arc<dyn Notifier>> {
    let mut out: Vec<Arc<dyn Notifier>> = Vec::new();
    if let Some(d) = &cfg.discord {
        match DiscordNotifier::new(d.webhook_url.expose()) {
            Ok(n) => out.push(Arc::new(n)),
            Err(e) => tracing::warn!(error = %e, "discord bildiricisi kurulamadı"),
        }
    }
    if let Some(t) = &cfg.telegram {
        match TelegramNotifier::new(t.bot_token.expose(), &t.chat_id) {
            Ok(n) => out.push(Arc::new(n)),
            Err(e) => tracing::warn!(error = %e, "telegram bildiricisi kurulamadı"),
        }
    }
    out
}

/// Tüm bildiricilere eşzamanlı gönderir; biri başarısız olunca diğerleri etkilenmez.
pub async fn send_all(notifiers: &[Arc<dyn Notifier>], n: &Notification) {
    let jobs = notifiers.iter().map(|nf| async move {
        if let Err(e) = nf.send(n).await {
            tracing::warn!(notifier = nf.name(), error = %e, "bildirim gönderilemedi");
        }
    });
    futures_util::future::join_all(jobs).await;
}

/// Ortak HTTP gönderici: rate limit + retry/backoff + zaman aşımı.
pub(crate) struct Sender {
    client: reqwest::Client,
    min_interval: Duration,
    backoff: Duration,
    last: Mutex<Option<Instant>>,
}

impl Sender {
    pub(crate) fn new() -> Result<Self, NotifyError> {
        Self::with_timing(MIN_INTERVAL, BACKOFF_BASE)
    }

    /// Testlerde kısa süreler vermek için.
    pub(crate) fn with_timing(
        min_interval: Duration,
        backoff: Duration,
    ) -> Result<Self, NotifyError> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|e| NotifyError::Request(e.without_url()))?;
        Ok(Self {
            client,
            min_interval,
            backoff,
            last: Mutex::new(None),
        })
    }

    /// Önceki gönderimden beri `min_interval` dolana kadar bekler.
    async fn throttle(&self) {
        let mut last = self.last.lock().await;
        if let Some(prev) = *last {
            tokio::time::sleep_until((prev + self.min_interval).into()).await;
        }
        *last = Some(Instant::now());
    }

    pub(crate) async fn post_json(&self, url: &str, body: &Value) -> Result<(), NotifyError> {
        let mut delay = self.backoff;
        let mut attempt = 1;
        loop {
            self.throttle().await;
            let (err, wait) = match self.client.post(url).json(body).send().await {
                Ok(r) if r.status().is_success() => return Ok(()),
                Ok(r) => {
                    let status = r.status();
                    if status.as_u16() == 429 {
                        let ra = retry_after(r).await;
                        (NotifyError::Status(429), ra.unwrap_or(delay))
                    } else if status.is_server_error() {
                        (NotifyError::Status(status.as_u16()), delay)
                    } else {
                        return Err(NotifyError::Status(status.as_u16()));
                    }
                }
                // URL (ve içindeki token) hatadan çıkarılır.
                Err(e) => (NotifyError::Request(e.without_url()), delay),
            };
            if attempt >= MAX_ATTEMPTS {
                return Err(err);
            }
            tokio::time::sleep(wait).await;
            delay *= 2;
            attempt += 1;
        }
    }
}

/// `Retry-After` başlığı, Discord `retry_after` veya Telegram
/// `parameters.retry_after` (saniye, ondalıklı olabilir).
async fn retry_after(r: reqwest::Response) -> Option<Duration> {
    let header = r
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.trim().parse::<f64>().ok());
    let secs = match header {
        Some(s) => s,
        None => {
            let v: Value = r.json().await.ok()?;
            v.get("retry_after")
                .or_else(|| v.get("parameters")?.get("retry_after"))?
                .as_f64()?
        }
    };
    Duration::try_from_secs_f64(secs)
        .ok()
        .map(|d| d.min(MAX_RETRY_AFTER))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    pub(crate) fn sample(kind: NotificationKind) -> Notification {
        Notification {
            kind,
            server: "web-1".into(),
            title: "Disk dolu".into(),
            body: "disk_usage > 90".into(),
            at: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
        }
    }

    fn fast() -> Sender {
        Sender::with_timing(Duration::ZERO, Duration::from_millis(20)).unwrap()
    }

    #[tokio::test]
    async fn retry_after_govdeden_okunur_sonra_basarili() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).set_body_json(json!({"retry_after": 0.3})))
            .up_to_n_times(1)
            .expect(1)
            .mount(&srv)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&srv)
            .await;
        let t = Instant::now();
        fast().post_json(&srv.uri(), &json!({})).await.unwrap();
        assert!(t.elapsed() >= Duration::from_millis(300));
    }

    #[tokio::test]
    async fn telegram_tarzi_retry_after_ve_baslik() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(429)
                    .set_body_json(json!({"ok": false, "parameters": {"retry_after": 0.2}})),
            )
            .up_to_n_times(1)
            .mount(&srv)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&srv)
            .await;
        let t = Instant::now();
        fast().post_json(&srv.uri(), &json!({})).await.unwrap();
        assert!(t.elapsed() >= Duration::from_millis(200));

        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0.2"))
            .up_to_n_times(1)
            .mount(&srv)
            .await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&srv)
            .await;
        let t = Instant::now();
        fast().post_json(&srv.uri(), &json!({})).await.unwrap();
        assert!(t.elapsed() >= Duration::from_millis(200));
    }

    #[tokio::test]
    async fn sunucu_hatasi_uc_denemeden_sonra_vazgecer() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500))
            .expect(3)
            .mount(&srv)
            .await;
        let e = fast().post_json(&srv.uri(), &json!({})).await.unwrap_err();
        assert!(matches!(e, NotifyError::Status(500)));
    }

    #[tokio::test]
    async fn istemci_hatasi_tekrar_denenmez() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(400))
            .expect(1)
            .mount(&srv)
            .await;
        let e = fast().post_json(&srv.uri(), &json!({})).await.unwrap_err();
        assert!(matches!(e, NotifyError::Status(400)));
    }

    #[tokio::test]
    async fn rate_limit_gonderimleri_aralar() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&srv)
            .await;
        let s = Sender::with_timing(Duration::from_millis(150), Duration::from_millis(20)).unwrap();
        let t = Instant::now();
        for _ in 0..3 {
            s.post_json(&srv.uri(), &json!({})).await.unwrap();
        }
        assert!(t.elapsed() >= Duration::from_millis(300));
    }

    #[tokio::test]
    async fn token_hata_metninde_gorunmez() {
        const TOKEN: &str = "SUPERSECRETTOKEN123";
        // Port 1: bağlantı reddedilir; URL yol kısmında sahte token var.
        let url = format!("http://127.0.0.1:1/bot{TOKEN}/sendMessage");
        let e = fast().post_json(&url, &json!({})).await.unwrap_err();
        assert!(matches!(e, NotifyError::Request(_)));
        assert!(!e.to_string().contains(TOKEN));
        assert!(!format!("{e:?}").contains(TOKEN));
    }

    #[tokio::test]
    async fn send_all_biri_basarisiz_olsa_digeri_gonderir() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .expect(1)
            .mount(&srv)
            .await;
        let bad: Arc<dyn Notifier> =
            Arc::new(DiscordNotifier::with_sender("http://127.0.0.1:1/x", fast()));
        let good: Arc<dyn Notifier> = Arc::new(DiscordNotifier::with_sender(&srv.uri(), fast()));
        send_all(&[bad, good], &sample(NotificationKind::Firing)).await;
    }
}
