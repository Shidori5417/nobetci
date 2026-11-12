//! `nobetci check`: her sunucuya bir kez bağlanıp toplu komutu çalıştırır.
//! known_hosts'a anahtar eklemenin tek izinli yeri burasıdır.

use std::io::{IsTerminal, Write};

use std::time::SystemTime;

use crate::config::Config;
use crate::notify::{self, Notification, NotificationKind};
use crate::parser;
use crate::ssh::{self, SshError};

const GIB: f64 = (1u64 << 30) as f64;

/// Bağlanır; bilinmeyen host ise (TTY'de ve kullanıcı onaylarsa) known_hosts'a ekleyip yeniden dener.
async fn connect_with_prompt(t: &ssh::Target) -> Result<ssh::Session, SshError> {
    match ssh::connect(t).await {
        Err(SshError::UnknownHost {
            host,
            port,
            fingerprint,
            key,
        }) => {
            eprintln!("  Bilinmeyen host anahtarı: {host}:{port}");
            eprintln!("  Parmak izi: {fingerprint}");
            if !std::io::stdin().is_terminal() {
                return Err(SshError::UnknownHost {
                    host,
                    port,
                    fingerprint,
                    key,
                });
            }
            eprint!("  Bu host anahtarını known_hosts'a ekle? [e/H] ");
            let _ = std::io::stderr().flush();
            let mut ans = String::new();
            std::io::stdin().read_line(&mut ans).ok();
            if !matches!(
                ans.trim().to_lowercase().as_str(),
                "e" | "evet" | "y" | "yes"
            ) {
                return Err(SshError::Other("host anahtarı reddedildi".into()));
            }
            ssh::learn_host(&host, port, &key)?;
            eprintln!("  known_hosts'a eklendi.");
            ssh::connect(t).await
        }
        r => r,
    }
}

async fn check_server(s: &crate::config::ServerConfig) -> Result<String, SshError> {
    let t = ssh::resolve(s)?;
    let session = connect_with_prompt(&t).await?;
    let out = session.exec(&parser::build_command(&s.services)).await?;
    let b = parser::parse_batch(&out);
    let na = || "?".to_string();
    let cores = b
        .stat
        .as_ref()
        .map_or_else(|_| na(), |c| c.cores.len().to_string());
    let mem = b.mem.as_ref().map_or_else(
        |_| na(),
        |m| {
            format!(
                "{:.1}/{:.1} GiB boş",
                m.available as f64 / GIB,
                m.total as f64 / GIB
            )
        },
    );
    let load = b
        .load
        .as_ref()
        .map_or_else(|_| na(), |l| format!("{:.2}", l.one));
    let svc = b.services.as_ref().map_or_else(
        |_| na(),
        |v| {
            v.iter()
                .map(|x| {
                    if x.is_missing() {
                        format!("{}=bulunamadı", x.name)
                    } else {
                        format!("{}={}", x.name, x.active_state)
                    }
                })
                .collect::<Vec<_>>()
                .join(", ")
        },
    );
    Ok(format!(
        "{}@{}:{}  CPU çekirdek: {cores}  RAM: {mem}  load: {load}  servisler: [{svc}]",
        t.user, t.host, t.port
    ))
}

/// Yapılandırılmış her bildiriciye test mesajı yollar; hepsi başarılıysa `true`.
async fn test_notify(cfg: &Config) -> bool {
    let notifiers = notify::from_config(&cfg.notify);
    let n = Notification {
        kind: NotificationKind::Resolved,
        server: "nobetci".into(),
        title: "🔔 Nöbetçi test bildirimi".into(),
        body: "Bildirim kanalı çalışıyor.".into(),
        at: SystemTime::now(),
    };
    if notifiers.is_empty() {
        println!("✗ bildirim: yapılandırılmış bildirici yok");
        return false;
    }
    let mut ok = true;
    for nf in &notifiers {
        match nf.send(&n).await {
            Ok(()) => println!("✓ bildirim {}: gönderildi", nf.name()),
            Err(e) => {
                ok = false;
                println!("✗ bildirim {}: {e}", nf.name());
            }
        }
    }
    ok
}

/// Tüm sunucuları sırayla dener; hepsi başarılıysa `true`.
pub async fn run(cfg: &Config, test_notify_too: bool) -> bool {
    println!("Config geçerli: {} sunucu", cfg.servers.len());
    println!("Uyarı kuralı: {}", cfg.alerts.len());
    // Yalnızca adlar yazılır; webhook/token asla.
    let mut names = Vec::new();
    if cfg.notify.discord.is_some() {
        names.push("discord");
    }
    if cfg.notify.telegram.is_some() {
        names.push("telegram");
    }
    println!(
        "Bildiriciler: {}",
        if names.is_empty() {
            "yok".to_string()
        } else {
            names.join(", ")
        }
    );
    let mut all_ok = true;
    for s in &cfg.servers {
        match check_server(s).await {
            Ok(line) => println!("✓ {}: {line}", s.name),
            Err(e) => {
                all_ok = false;
                println!("✗ {}: {e}", s.name);
            }
        }
    }
    if test_notify_too {
        all_ok &= test_notify(cfg).await;
    }
    all_ok
}
