//! Discord webhook (embed).

use async_trait::async_trait;
use serde_json::{Value, json};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use super::{Notification, NotificationKind, Notifier, NotifyError, Sender};

const RED: u32 = 0xE74C3C;
const GREEN: u32 = 0x2ECC71;
/// Discord sınırları: başlık 256, açıklama 4096 karakter.
const TITLE_MAX: usize = 256;
const DESC_MAX: usize = 4096;

pub struct DiscordNotifier {
    // Token içerir; Debug türetilmez, loglanmaz.
    url: String,
    sender: Sender,
}

impl DiscordNotifier {
    pub fn new(webhook_url: &str) -> Result<Self, NotifyError> {
        Ok(Self::with_sender(webhook_url, Sender::new()?))
    }

    /// Testlerde mock URL ve kısa süreler için.
    pub(crate) fn with_sender(webhook_url: &str, sender: Sender) -> Self {
        Self {
            url: webhook_url.to_owned(),
            sender,
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    s.chars().take(max).collect()
}

fn payload(n: &Notification) -> Value {
    let color = match n.kind {
        NotificationKind::Firing => RED,
        NotificationKind::Resolved => GREEN,
    };
    let ts = OffsetDateTime::from(n.at)
        .format(&Rfc3339)
        .unwrap_or_default();
    json!({
        "embeds": [{
            "title": truncate(&n.title, TITLE_MAX),
            "description": truncate(&n.body, DESC_MAX),
            "color": color,
            "timestamp": ts,
            "footer": { "text": "Nöbetçi" },
            "fields": [{ "name": "Sunucu", "value": truncate(&n.server, 1024), "inline": true }],
        }],
        // Sunucu kaynaklı metin @everyone/@here ile ping atamasın.
        "allowed_mentions": { "parse": [] },
    })
}

#[async_trait]
impl Notifier for DiscordNotifier {
    fn name(&self) -> &'static str {
        "discord"
    }

    async fn send(&self, n: &Notification) -> Result<(), NotifyError> {
        self.sender.post_json(&self.url, &payload(n)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::tests::sample;
    use std::time::Duration;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn body_for(kind: NotificationKind) -> Value {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(204))
            .mount(&srv)
            .await;
        let s = Sender::with_timing(Duration::ZERO, Duration::from_millis(10)).unwrap();
        DiscordNotifier::with_sender(&srv.uri(), s)
            .send(&sample(kind))
            .await
            .unwrap();
        let reqs = srv.received_requests().await.unwrap();
        serde_json::from_slice(&reqs[0].body).unwrap()
    }

    #[tokio::test]
    async fn embed_sekli_ve_renk() {
        let v = body_for(NotificationKind::Firing).await;
        let e = &v["embeds"][0];
        assert_eq!(e["title"], "Disk dolu");
        assert_eq!(e["description"], "disk_usage > 90");
        assert_eq!(e["color"], RED);
        assert_eq!(e["footer"]["text"], "Nöbetçi");
        assert_eq!(e["timestamp"], "2023-11-14T22:13:20Z");
        assert_eq!(e["fields"][0]["value"], "web-1");

        let v = body_for(NotificationKind::Resolved).await;
        assert_eq!(v["embeds"][0]["color"], GREEN);
    }
}
