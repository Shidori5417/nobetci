//! Telegram Bot API (`sendMessage`, HTML parse_mode).

use async_trait::async_trait;
use serde_json::{Value, json};

use super::{Notification, NotificationKind, Notifier, NotifyError, Sender};

const API_BASE: &str = "https://api.telegram.org";
/// Telegram mesaj sınırı 4096 karakter; kaçışlı metin bu sınırı aşmaz.
const TEXT_MAX: usize = 4096;

pub struct TelegramNotifier {
    // Token içerir; Debug türetilmez, loglanmaz.
    token: String,
    chat_id: String,
    base: String,
    sender: Sender,
}

impl TelegramNotifier {
    pub fn new(bot_token: &str, chat_id: &str) -> Result<Self, NotifyError> {
        Ok(Self::with_base(
            API_BASE,
            bot_token,
            chat_id,
            Sender::new()?,
        ))
    }

    /// Testlerde mock taban URL ve kısa süreler için.
    pub(crate) fn with_base(base: &str, bot_token: &str, chat_id: &str, sender: Sender) -> Self {
        Self {
            token: bot_token.to_owned(),
            chat_id: chat_id.to_owned(),
            base: base.trim_end_matches('/').to_owned(),
            sender,
        }
    }
}

/// Sunucudan gelen metin güvenilmez: HTML özel karakterlerini kaçırır.
fn escape_html(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '&' => out.push_str("&amp;"),
            c => out.push(c),
        }
    }
}

fn text(n: &Notification) -> String {
    let icon = match n.kind {
        NotificationKind::Firing => "🔴",
        NotificationKind::Resolved => "🟢",
    };
    let mut t = format!("{icon} <b>");
    escape_html(&n.title, &mut t);
    t.push_str("</b>\n<i>");
    escape_html(&n.server, &mut t);
    t.push_str("</i>\n\n");
    // Gövde sınıra kadar eklenir; kaçış dizisi ortasından kesilmez.
    for c in n.body.chars() {
        let mut esc = String::new();
        escape_html(c.encode_utf8(&mut [0; 4]), &mut esc);
        if t.chars().count() + esc.chars().count() > TEXT_MAX {
            break;
        }
        t.push_str(&esc);
    }
    t
}

fn payload(chat_id: &str, n: &Notification) -> Value {
    json!({
        "chat_id": chat_id,
        "text": text(n),
        "parse_mode": "HTML",
        "disable_web_page_preview": true,
    })
}

#[async_trait]
impl Notifier for TelegramNotifier {
    fn name(&self) -> &'static str {
        "telegram"
    }

    async fn send(&self, n: &Notification) -> Result<(), NotifyError> {
        let url = format!("{}/bot{}/sendMessage", self.base, self.token);
        self.sender
            .post_json(&url, &payload(&self.chat_id, n))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::tests::sample;
    use std::time::Duration;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn payload_yol_ve_kacis() {
        let srv = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/botTOK/sendMessage"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&srv)
            .await;
        let s = Sender::with_timing(Duration::ZERO, Duration::from_millis(10)).unwrap();
        let tg = TelegramNotifier::with_base(&srv.uri(), "TOK", "-100", s);
        let mut n = sample(NotificationKind::Firing);
        n.body = "<script>a & b</script>".into();
        tg.send(&n).await.unwrap();

        let reqs = srv.received_requests().await.unwrap();
        let v: Value = serde_json::from_slice(&reqs[0].body).unwrap();
        assert_eq!(v["chat_id"], "-100");
        assert_eq!(v["parse_mode"], "HTML");
        assert_eq!(v["disable_web_page_preview"], true);
        let t = v["text"].as_str().unwrap();
        assert!(t.contains("&lt;script&gt;a &amp; b&lt;/script&gt;"));
        assert!(!t.contains("<script>"));
        assert!(t.contains("<b>Disk dolu</b>"));
    }

    #[test]
    fn uzun_metin_sinira_kirpilir() {
        let mut n = sample(NotificationKind::Resolved);
        n.body = "&".repeat(10_000);
        let t = text(&n);
        assert!(t.chars().count() <= TEXT_MAX);
        assert!(t.ends_with("&amp;"));
    }
}
