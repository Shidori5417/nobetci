//! Saf Rust SSH istemcisi (`russh`). Sistemdeki `ssh` binary'si asla çağrılmaz.
//!
//! VARSAYIM: `russh-keys` crate'i `russh` içine birleşti (`russh::keys`); ayrı
//! bir `russh-keys` bağımlılığı eklenmedi.
//!
//! Güvenlik: sunucu anahtarı `~/.ssh/known_hosts` ile doğrulanır. Bilinmeyen host
//! reddedilir (parmak izi hatada yer alır), değişmiş anahtar her zaman reddedilir.
//! Bu modül known_hosts'a ASLA yazmaz; yalnızca `nobetci check` içindeki açık
//! onay `learn_host` çağırır.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::ChannelMsg;
use russh::client::{self, Handle};
use russh::keys::agent::AgentIdentity;
use russh::keys::agent::client::AgentClient;
use russh::keys::{HashAlg, PrivateKeyWithHashAlg, PublicKey};

use crate::config::ServerConfig;

/// Bağlantı kurma + kimlik doğrulama için toplam süre sınırı.
// VARSAYIM: 10 sn yavaş hatlar için yeterli, ölü sunucuda UI'yi uzun beklemez.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Tek bir exec için süre sınırı.
pub const EXEC_TIMEOUT: Duration = Duration::from_secs(10);
/// Sunucu çıktısı güvenilmezdir; bundan fazlası hata sayılır.
pub const MAX_OUTPUT: usize = 1 << 20;

#[derive(Debug, thiserror::Error)]
pub enum SshError {
    #[error(
        "bilinmeyen host anahtarı {host}:{port} ({fingerprint}); `nobetci check` ile onaylayın"
    )]
    UnknownHost {
        host: String,
        port: u16,
        fingerprint: String,
        key: Box<PublicKey>,
    },
    #[error(
        "UYARI: {host}:{port} HOST ANAHTARI DEĞİŞMİŞ (known_hosts satır {line}, yeni parmak izi {fingerprint}). Ortadaki adam saldırısı olabilir, bağlantı reddedildi"
    )]
    HostKeyChanged {
        host: String,
        port: u16,
        line: usize,
        fingerprint: String,
    },
    #[error("kimlik doğrulama başarısız: {0}")]
    Auth(String),
    #[error("{0}")]
    Other(String),
}

fn other(e: impl std::fmt::Display) -> SshError {
    SshError::Other(e.to_string())
}

/// Bir sunucunun çözülmüş bağlantı bilgileri.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub identities: Vec<PathBuf>,
    /// `false` ise `identities` varsayılan adaylardır; olmayanlar atlanır.
    pub explicit_identities: bool,
}

fn expand_tilde(p: &str, home: &Path) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(p),
    }
}

/// Saf çözümleme: ssh_config metni, home ve `$USER` dışarıdan verilir.
pub fn resolve_with(
    server: &ServerConfig,
    ssh_config: Option<&str>,
    home: &Path,
    env_user: Option<&str>,
) -> Result<Target, SshError> {
    let (mut host, mut port, mut user, mut ids) = (None, None, None, Vec::new());
    if let Some(alias) = &server.ssh_host {
        let cfg = russh_config::parse(ssh_config.unwrap_or(""), alias)
            .map_err(|e| other(format!("ssh config ({alias}): {e}")))?;
        host = Some(cfg.host().to_string());
        port = cfg.host_config.port;
        user = cfg.host_config.user.clone();
        ids = cfg.host_config.identity_file.clone().unwrap_or_default();
    } else {
        host.clone_from(&server.host);
    }
    // Açık config alanları ssh_config'i geçersiz kılar.
    let port = server.port.or(port).unwrap_or(22);
    let user = server
        .user
        .clone()
        .or(user)
        .or_else(|| env_user.map(String::from))
        .ok_or_else(|| other("kullanıcı adı belirlenemedi ($USER yok)"))?;
    let host = host.ok_or_else(|| other("host belirlenemedi"))?;
    if let Some(k) = &server.key {
        ids = vec![expand_tilde(k, home)];
    } else {
        ids = ids
            .iter()
            .map(|p| expand_tilde(&p.to_string_lossy(), home))
            .collect();
    }
    let explicit_identities = !ids.is_empty();
    if !explicit_identities {
        ids = ["id_ed25519", "id_ecdsa", "id_rsa"]
            .iter()
            .map(|n| home.join(".ssh").join(n))
            .collect();
    }
    Ok(Target {
        host,
        port,
        user,
        identities: ids,
        explicit_identities,
    })
}

/// `HOME` ortam değişkenine (directories üzerinden) göre çözümler.
pub fn resolve(server: &ServerConfig) -> Result<Target, SshError> {
    let home = directories::BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .ok_or_else(|| other("ev dizini bulunamadı"))?;
    let text = std::fs::read_to_string(home.join(".ssh").join("config")).ok();
    let user = std::env::var("USER").ok();
    resolve_with(server, text.as_deref(), &home, user.as_deref())
}

/// Host anahtarı doğrulama sonucunu bağlantı hatasından ayırmak için paylaşılan yuva.
type Verdict = Arc<Mutex<Option<SshError>>>;

struct Client {
    host: String,
    port: u16,
    verdict: Verdict,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // VARSAYIM: sunucu sertifikaları (CA ile imzalı host anahtarı) desteklenmez.
        let russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } = key else {
            *self.verdict.lock().unwrap() = Some(other("host sertifikaları desteklenmiyor"));
            return Ok(false);
        };
        let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
        let err = match russh::keys::check_known_hosts(&self.host, self.port, key) {
            Ok(true) => return Ok(true),
            Ok(false) => SshError::UnknownHost {
                host: self.host.clone(),
                port: self.port,
                fingerprint,
                key: Box::new(key.clone()),
            },
            Err(russh::keys::Error::KeyChanged { line }) => SshError::HostKeyChanged {
                host: self.host.clone(),
                port: self.port,
                line,
                fingerprint,
            },
            Err(e) => other(format!("known_hosts okunamadı: {e}")),
        };
        *self.verdict.lock().unwrap() = Some(err);
        Ok(false)
    }
}

/// Açık bir SSH oturumu. Tek oturum açık tutulur, her tick yeni kanal açar.
pub struct Session {
    handle: Handle<Client>,
}

/// known_hosts'a ekler. YALNIZCA `nobetci check` içindeki açık onaydan sonra çağrılır.
pub fn learn_host(host: &str, port: u16, key: &PublicKey) -> Result<(), SshError> {
    russh::keys::known_hosts::learn_known_hosts(host, port, key).map_err(other)
}

pub async fn connect(t: &Target) -> Result<Session, SshError> {
    tokio::time::timeout(CONNECT_TIMEOUT, connect_inner(t))
        .await
        .map_err(|_| other(format!("bağlantı zaman aşımı ({}:{})", t.host, t.port)))?
}

async fn connect_inner(t: &Target) -> Result<Session, SshError> {
    let verdict: Verdict = Arc::default();
    let config = Arc::new(client::Config {
        inactivity_timeout: None,
        // Kopan bağlantı keepalive ile fark edilir.
        keepalive_interval: Some(Duration::from_secs(15)),
        keepalive_max: 3,
        ..Default::default()
    });
    let handler = Client {
        host: t.host.clone(),
        port: t.port,
        verdict: verdict.clone(),
    };
    let mut handle =
        match client::connect(config, (t.host.as_str(), t.port), handler).await {
            Ok(h) => h,
            Err(e) => {
                // Host anahtarı reddi, genel "UnknownKey" hatasından daha açıklayıcıdır.
                return Err(verdict.lock().unwrap().take().unwrap_or_else(|| {
                    other(format!("{}:{} bağlantı hatası: {e}", t.host, t.port))
                }));
            }
        };
    authenticate(&mut handle, t).await?;
    Ok(Session { handle })
}

/// Önce ssh-agent (varsa), sonra anahtar dosyaları. Parola yok, istem yok.
async fn authenticate(handle: &mut Handle<Client>, t: &Target) -> Result<(), SshError> {
    let mut tried: Vec<String> = Vec::new();
    let rsa_hash = handle
        .best_supported_rsa_hash()
        .await
        .map_err(other)?
        .flatten();

    if std::env::var_os("SSH_AUTH_SOCK").is_some() {
        match AgentClient::connect_env().await {
            Ok(mut agent) => match agent.request_identities().await {
                Ok(ids) => {
                    for id in ids {
                        let AgentIdentity::PublicKey { key, .. } = id else {
                            continue;
                        };
                        match handle
                            .authenticate_publickey_with(&t.user, key, rsa_hash, &mut agent)
                            .await
                        {
                            Ok(r) if r.success() => return Ok(()),
                            Ok(_) => {}
                            Err(e) => tried.push(format!("agent: {e}")),
                        }
                    }
                    tried.push("agent: kabul edilen anahtar yok".into());
                }
                Err(e) => tried.push(format!("agent: {e}")),
            },
            Err(e) => tried.push(format!("agent: {e}")),
        }
    }

    for path in &t.identities {
        if !path.exists() {
            if t.explicit_identities {
                tried.push(format!("{}: dosya yok", path.display()));
            }
            continue;
        }
        let key = match russh::keys::load_secret_key(path, None) {
            Ok(k) => k,
            Err(russh::keys::Error::KeyIsEncrypted) => {
                tried.push(format!(
                    "{}: anahtar parolalı ve ssh-agent yok (parola istenmez)",
                    path.display()
                ));
                continue;
            }
            Err(e) => {
                tried.push(format!("{}: {e}", path.display()));
                continue;
            }
        };
        let key = PrivateKeyWithHashAlg::new(Arc::new(key), rsa_hash);
        match handle.authenticate_publickey(&t.user, key).await {
            Ok(r) if r.success() => return Ok(()),
            Ok(_) => tried.push(format!("{}: sunucu reddetti", path.display())),
            Err(e) => tried.push(format!("{}: {e}", path.display())),
        }
    }
    if tried.is_empty() {
        tried.push("kullanılabilir anahtar bulunamadı".into());
    }
    Err(SshError::Auth(tried.join("; ")))
}

impl Session {
    /// Komutu çalıştırıp stdout'u döner. Süre ve boyut sınırlıdır.
    pub async fn exec(&self, cmd: &str) -> Result<String, SshError> {
        tokio::time::timeout(EXEC_TIMEOUT, self.exec_inner(cmd))
            .await
            .map_err(|_| other("komut zaman aşımı"))?
    }

    async fn exec_inner(&self, cmd: &str) -> Result<String, SshError> {
        let mut ch = self.handle.channel_open_session().await.map_err(other)?;
        ch.exec(true, cmd).await.map_err(other)?;
        let mut out: Vec<u8> = Vec::new();
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => {
                    if out.len() + data.len() > MAX_OUTPUT {
                        return Err(other("çıktı boyut sınırını aştı (1 MiB)"));
                    }
                    out.extend_from_slice(&data);
                }
                ChannelMsg::Eof | ChannelMsg::Close => break,
                _ => {}
            }
        }
        // Geçersiz UTF-8 çökertmez, değiştirme karakteriyle okunur.
        Ok(String::from_utf8_lossy(&out).into_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::keys::ssh_key;

    fn server(toml: &str) -> ServerConfig {
        crate::config::Config::parse(&format!("[[server]]\nname=\"s\"\n{toml}"))
            .unwrap()
            .servers
            .remove(0)
    }

    const CFG: &str = "Host vds1\n  HostName 10.0.0.5\n  Port 2200\n  User deploy\n  IdentityFile /keys/vds_key\n";
    const HOME: &str = "/home/u";

    #[test]
    fn ssh_config_alias_cozulur() {
        let t = resolve_with(
            &server("ssh_host=\"vds1\""),
            Some(CFG),
            Path::new(HOME),
            Some("u"),
        )
        .unwrap();
        assert_eq!(t.host, "10.0.0.5");
        assert_eq!(t.port, 2200);
        assert_eq!(t.user, "deploy");
        assert_eq!(t.identities, vec![PathBuf::from("/keys/vds_key")]);
        assert!(t.explicit_identities);
    }

    #[test]
    fn acik_alanlar_ssh_configi_gecersiz_kilar() {
        let s = server("ssh_host=\"vds1\"\nuser=\"root\"\nport=22\nkey=\"~/k\"");
        let t = resolve_with(&s, Some(CFG), Path::new(HOME), Some("u")).unwrap();
        assert_eq!((t.port, t.user.as_str()), (22, "root"));
        assert_eq!(t.identities, vec![PathBuf::from("/home/u/k")]);
    }

    #[test]
    fn bilinmeyen_alias_ve_varsayilanlar() {
        let t = resolve_with(
            &server("ssh_host=\"yok\""),
            Some(CFG),
            Path::new(HOME),
            Some("u"),
        )
        .unwrap();
        assert_eq!((t.host.as_str(), t.port, t.user.as_str()), ("yok", 22, "u"));
        assert!(!t.explicit_identities);
        assert_eq!(t.identities.len(), 3);
        assert!(t.identities[0].ends_with(".ssh/id_ed25519"));
    }

    #[test]
    fn acik_host_ve_ssh_config_yok() {
        let t = resolve_with(
            &server("host=\"h.example\""),
            None,
            Path::new(HOME),
            Some("u"),
        )
        .unwrap();
        assert_eq!((t.host.as_str(), t.port), ("h.example", 22));
    }

    #[test]
    fn kullanici_yoksa_hata() {
        assert!(resolve_with(&server("host=\"h\""), None, Path::new(HOME), None).is_err());
    }

    #[test]
    fn known_hosts_port_bicimi() {
        let key = ssh_key::PublicKey::from_openssh(
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl",
        )
        .unwrap();
        let path = std::env::temp_dir().join(format!("nobetci_kh_{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        for port in [22, 2222] {
            russh::keys::known_hosts::learn_known_hosts_path("h.example", port, &key, &path)
                .unwrap();
        }
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.lines().any(|l| l.starts_with("h.example ssh-ed25519")));
        assert!(
            text.lines()
                .any(|l| l.starts_with("[h.example]:2222 ssh-ed25519"))
        );
        let check = |p| russh::keys::check_known_hosts_path("h.example", p, &key, &path).unwrap();
        assert!(check(22) && check(2222) && !check(2223));
        let _ = std::fs::remove_file(&path);
    }
}
