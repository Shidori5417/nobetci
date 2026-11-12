//! Config okuma, doğrulama ve örnek config.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Deserializer};

/// Varsayılan örnekleme aralığı.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(2);

/// `nobetci init` ile yazılan örnek config.
pub const EXAMPLE_CONFIG: &str = include_str!("../config.example.toml");

/// Config katmanı hataları.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("config dosyası okunamadı ({path}): {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("config ayrıştırılamadı: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("sunucu adı birden fazla kez kullanılmış: {0}")]
    DuplicateServer(String),
    #[error("sunucu '{0}' için ssh_host veya host gerekli")]
    MissingHost(String),
    #[error("geçersiz süre '{0}' (örnek: 30s, 5m, 1h)")]
    InvalidDuration(String),
    #[error("geçersiz uyarı kuralı {0}")]
    InvalidAlert(String),
    #[error("uyarı kuralı '{rule}' tanımsız sunucuyu adlandırıyor: {server}")]
    UnknownAlertServer { rule: String, server: String },
}

/// Loglara sızmaması için `Debug` çıktısı `***` olan gizli değer.
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    /// Gerçek değer; yalnızca bildirimi gönderirken kullanılmalı.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    /// Örnekleme aralığı (`interval = "2s"`); yoksa `DEFAULT_INTERVAL`.
    #[serde(default, deserialize_with = "de_duration")]
    pub interval: Option<Duration>,
    #[serde(default, rename = "server")]
    pub servers: Vec<ServerConfig>,
    #[serde(default)]
    pub notify: NotifyConfig,
    #[serde(default, rename = "alert")]
    pub alerts: Vec<AlertRule>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerConfig {
    pub name: String,
    pub ssh_host: Option<String>,
    pub host: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    pub key: Option<String>,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub allow_actions: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct NotifyConfig {
    pub discord: Option<DiscordConfig>,
    pub telegram: Option<TelegramConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DiscordConfig {
    pub webhook_url: Secret,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TelegramConfig {
    pub bot_token: Secret,
    pub chat_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AlertRule {
    /// Kural ifadesi; `alert::AlertRule::parse` ile ayrıştırılır, `validate` doğrular.
    pub when: String,
    #[serde(default, rename = "for", deserialize_with = "de_duration")]
    pub for_duration: Option<Duration>,
    #[serde(default)]
    pub servers: Vec<String>,
}

fn de_duration<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Duration>, D::Error> {
    let s = Option::<String>::deserialize(d)?;
    s.map(|s| parse_duration(&s).map_err(serde::de::Error::custom))
        .transpose()
}

/// "30s", "5m", "1h", "2d" biçimindeki süreyi ayrıştırır.
pub fn parse_duration(s: &str) -> Result<Duration, ConfigError> {
    let bad = || ConfigError::InvalidDuration(s.to_string());
    let t = s.trim();
    let unit = t.chars().last().ok_or_else(bad)?;
    let mult: u64 = match unit {
        's' => 1,
        'm' => 60,
        'h' => 3600,
        'd' => 86400,
        _ => return Err(bad()),
    };
    let n: u64 = t[..t.len() - 1].trim().parse().map_err(|_| bad())?;
    Ok(Duration::from_secs(n.checked_mul(mult).ok_or_else(bad)?))
}

impl Config {
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let cfg: Config = toml::from_str(text)?;
        cfg.validate()?;
        Ok(cfg)
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text)
    }

    /// Etkin örnekleme aralığı.
    // VARSAYIM: 1 saniyenin altı sunucuyu gereksiz yorar, alt sınır 1s.
    pub fn interval(&self) -> Duration {
        self.interval
            .unwrap_or(DEFAULT_INTERVAL)
            .max(Duration::from_secs(1))
    }

    fn validate(&self) -> Result<(), ConfigError> {
        let mut seen = HashSet::new();
        for s in &self.servers {
            if !seen.insert(s.name.as_str()) {
                return Err(ConfigError::DuplicateServer(s.name.clone()));
            }
            if s.ssh_host.is_none() && s.host.is_none() {
                return Err(ConfigError::MissingHost(s.name.clone()));
            }
        }
        crate::alert::validate(&self.alerts, &self.servers)
    }
}

/// Varsayılan config yolu: `~/.config/nobetci/config.toml`.
pub fn default_config_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "nobetci").map(|d| d.config_dir().join("config.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const README_EXAMPLE: &str = r#"
[[server]]
name = "vds-1"
ssh_host = "vds1"
services = ["nginx", "minecraft", "postgresql"]

[[server]]
name = "yedek"
host = "203.0.113.10"
user = "root"
port = 22
key = "~/.ssh/id_ed25519"

[notify.discord]
webhook_url = "https://discord.com/api/webhooks/..."

[notify.telegram]
bot_token = "..."
chat_id = "..."

[[alert]]
when = "disk_usage > 90"
for = "5m"
servers = ["*"]

[[alert]]
when = "service_down"
servers = ["vds-1"]
"#;

    #[test]
    fn readme_ornegi_ayristirilir() {
        let c = Config::parse(README_EXAMPLE).unwrap();
        assert_eq!(c.servers.len(), 2);
        assert_eq!(c.servers[0].services.len(), 3);
        assert!(!c.servers[0].allow_actions);
        assert_eq!(c.servers[1].port, Some(22));
        assert!(c.notify.discord.is_some() && c.notify.telegram.is_some());
        assert_eq!(c.alerts[0].for_duration, Some(Duration::from_secs(300)));
        assert_eq!(c.alerts[1].for_duration, None);
    }

    #[test]
    fn gomulu_ornek_config_gecerli() {
        Config::parse(EXAMPLE_CONFIG).unwrap();
    }

    #[test]
    fn sure_ayristirma() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3600));
        for bad in ["", "5", "m", "5x", "-1m", "1.5h"] {
            assert!(parse_duration(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn gecersiz_sure_config_hatasi() {
        let t = "[[alert]]\nwhen = \"x\"\nfor = \"abc\"\n";
        assert!(matches!(Config::parse(t), Err(ConfigError::Parse(_))));
    }

    #[test]
    fn tekrar_eden_sunucu_adi_reddedilir() {
        let t = "[[server]]\nname=\"a\"\nhost=\"h\"\n[[server]]\nname=\"a\"\nhost=\"h2\"\n";
        assert!(matches!(
            Config::parse(t),
            Err(ConfigError::DuplicateServer(n)) if n == "a"
        ));
    }

    #[test]
    fn host_eksik_reddedilir() {
        let t = "[[server]]\nname=\"a\"\n";
        assert!(matches!(
            Config::parse(t),
            Err(ConfigError::MissingHost(n)) if n == "a"
        ));
    }

    #[test]
    fn gizli_degerler_debug_ta_gorunmez() {
        let c = Config::parse(README_EXAMPLE).unwrap();
        let dbg = format!("{c:?}");
        assert!(!dbg.contains("discord.com/api/webhooks"));
        assert!(dbg.contains("***"));
        let tg = c.notify.telegram.unwrap();
        assert_eq!(format!("{:?}", tg.bot_token), "***");
        assert_eq!(tg.bot_token.expose(), "...");
    }
}
