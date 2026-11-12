//! Canlı `journalctl -f` akışı. Sunucu çıktısı güvenilmezdir: satırlar ekrana
//! gitmeden önce ANSI kaçış dizilerinden ve kontrol karakterlerinden arındırılır.
//!
//! Akış, ana collector oturumundan bağımsız, kısa ömürlü kendi SSH oturumunu
//! kullanır (VARSAYIM: izleme döngüsüne komut kanalı eklemekten daha basit ve
//! log izleyici kapanınca oturum da kapanır). Alıcı (`mpsc::Receiver`) düşürülünce
//! görev uzaktaki komutu durdurur ve oturumu kapatır.

use std::time::Duration;

use russh::ChannelMsg;
use tokio::sync::mpsc;

use crate::config::ServerConfig;
use crate::parser::valid_service_name;
use crate::ssh;

/// Tek bir satır bundan uzunsa bölünür (sunucu `\n` göndermeyebilir).
const MAX_LINE: usize = 16 * 1024;
/// Kapanışta uzak komutu durdurmak için beklenecek en uzun süre.
const CLEANUP_TIMEOUT: Duration = Duration::from_secs(2);

/// Akıştan UI'ye giden iletiler.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogMsg {
    /// Temizlenmiş yeni satırlar.
    Lines(Vec<String>),
    /// Bağlantı kurulamadı ya da akış sona erdi; bundan sonra ileti gelmez.
    Error(String),
}

/// ANSI kaçış dizilerini ve kontrol karakterlerini atar (sekme boşluğa çevrilir).
pub fn sanitize_line(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '\x1b' => match it.next() {
                // CSI: parametre/ara baytlar, sonra 0x40..=0x7e son bayt.
                Some('[') => {
                    for n in it.by_ref() {
                        if ('\x40'..='\x7e').contains(&n) {
                            break;
                        }
                    }
                }
                // OSC/DCS/PM/APC...: BEL ya da ST (ESC \) ile biter.
                Some(']' | 'P' | 'X' | '^' | '_') => {
                    while let Some(n) = it.next() {
                        if n == '\x07' {
                            break;
                        }
                        if n == '\x1b' {
                            it.next();
                            break;
                        }
                    }
                }
                // Ara baytlı (ESC ( B gibi) ya da tek karakterlik dizi.
                Some(n) if ('\x20'..='\x2f').contains(&n) => {
                    it.next();
                }
                _ => {}
            },
            '\t' => out.push(' '),
            // C0/C1 kontrolleri, DEL ve çift yönlü metin aldatmacası karakterleri.
            c if c.is_control() || ('\u{202a}'..='\u{202e}').contains(&c) => {}
            c if ('\u{2066}'..='\u{2069}').contains(&c) => {}
            c => out.push(c),
        }
    }
    out
}

/// Bayt akışını satırlara böler; yarım satır bir sonraki parçaya kalır.
#[derive(Default)]
struct LineSplitter(Vec<u8>);

impl LineSplitter {
    fn push(&mut self, data: &[u8]) -> Vec<String> {
        let mut lines = Vec::new();
        for &b in data {
            if b == b'\n' || self.0.len() >= MAX_LINE {
                lines.push(sanitize_line(&String::from_utf8_lossy(&self.0)));
                self.0.clear();
                if b == b'\n' {
                    continue;
                }
            }
            self.0.push(b);
        }
        lines
    }
}

/// Uzakta çalışacak komut. `sh -c` sarmalayıcısı kullanıcının giriş kabuğundan bağımsızdır.
///
/// `journalctl` arka planda çalışır; stdin'i izleyen bir yardımcı, bağlantı/kanal
/// kapanıp stdin EOF olunca `journalctl`'i öldürür. PTY kullanılmadığı için
/// bu olmadan boştaki `journalctl -f` oturum kapandıktan sonra da yaşar.
pub fn journal_command(unit: Option<&str>) -> Result<String, String> {
    let unit = match unit {
        Some(u) if valid_service_name(u) => format!(" -u {u}"),
        Some(u) => return Err(format!("geçersiz servis adı: {u:?}")),
        None => String::new(),
    };
    Ok(format!(
        "sh -c 'LC_ALL=C; export LC_ALL; exec 3<&0; \
         journalctl -f -n 200 --no-pager -o short-iso{unit} & p=$!; \
         (exec >/dev/null 2>&1; cat <&3; kill $p) & w=$!; \
         wait $p; r=$?; kill $w 2>/dev/null; exit $r'"
    ))
}

/// Gerçek sunucudan akış başlatır. Dönen alıcı düşürülünce akış durur.
pub fn spawn(server: ServerConfig, unit: Option<String>) -> mpsc::Receiver<LogMsg> {
    let (tx, rx) = mpsc::channel(256);
    tokio::spawn(async move {
        if let Err(e) = stream(&server, unit.as_deref(), &tx).await {
            tracing::warn!("{}: log akışı: {e}", server.name);
            let _ = tx.send(LogMsg::Error(e)).await;
        }
    });
    rx
}

async fn stream(
    server: &ServerConfig,
    unit: Option<&str>,
    tx: &mpsc::Sender<LogMsg>,
) -> Result<(), String> {
    let cmd = journal_command(unit)?;
    let target = ssh::resolve(server).map_err(|e| e.to_string())?;
    let session = tokio::select! {
        _ = tx.closed() => return Ok(()),
        s = ssh::connect(&target) => s.map_err(|e| e.to_string())?,
    };
    let mut ch = session.open_exec(&cmd).await.map_err(|e| e.to_string())?;
    tracing::info!("{}: log akışı başladı", server.name);
    let (mut out, mut err) = (LineSplitter::default(), LineSplitter::default());
    let mut status = None;
    loop {
        let msg = tokio::select! {
            _ = tx.closed() => break,
            m = ch.wait() => m,
        };
        let lines = match msg {
            None | Some(ChannelMsg::Close) => break,
            Some(ChannelMsg::Data { data }) => out.push(&data),
            // journalctl hataları (ör. yetki yok) stderr'den gelir; "! " ile işaretlenir.
            Some(ChannelMsg::ExtendedData { data, .. }) => err
                .push(&data)
                .into_iter()
                .map(|l| format!("! {l}"))
                .collect(),
            Some(ChannelMsg::ExitStatus { exit_status }) => {
                status = Some(exit_status);
                continue;
            }
            Some(_) => continue,
        };
        if !lines.is_empty() && tx.send(LogMsg::Lines(lines)).await.is_err() {
            break;
        }
    }
    // Uzak journalctl'i durdur: EOF -> yardımcı kill eder; sonra kanal ve oturum kapanır.
    let _ = tokio::time::timeout(CLEANUP_TIMEOUT, async {
        let _ = ch.eof().await;
        let _ = ch.close().await;
        session.disconnect().await;
    })
    .await;
    tracing::info!("{}: log akışı kapandı", server.name);
    if tx.is_closed() {
        return Ok(());
    }
    Err(match status {
        Some(c) => format!("journalctl sona erdi (çıkış kodu {c})"),
        None => "bağlantı kapandı".into(),
    })
}

/// `--demo` için sahte akış; satırlar da aynı temizleme yolundan geçer.
pub fn spawn_demo(server: String, unit: Option<String>) -> mpsc::Receiver<LogMsg> {
    const MSGS: [&str; 5] = [
        "Started session of user deploy.",
        "connection from 203.0.113.7 accepted",
        "\x1b[31mhata: bağlantı sıfırlandı\x1b[0m",
        "cache temizlendi (42 kayıt)",
        "healthcheck ok",
    ];
    let (tx, rx) = mpsc::channel(256);
    tokio::spawn(async move {
        let name = unit.as_deref().unwrap_or("systemd");
        let mut n = 0u64;
        let mut iv = tokio::time::interval(Duration::from_millis(400));
        loop {
            iv.tick().await;
            // İlk turda geçmiş satırlar toplu gelir (gerçek `-n 200` gibi).
            let count = if n == 0 { 20 } else { 1 };
            let lines = (0..count)
                .map(|_| {
                    n += 1;
                    sanitize_line(&format!(
                        "2026-11-12T10:{:02}:{:02}+0300 {server} {name}[{}]: {} #{n}",
                        (n / 60) % 60,
                        n % 60,
                        1000 + n % 7,
                        MSGS[(n % MSGS.len() as u64) as usize]
                    ))
                })
                .collect();
            if tx.send(LogMsg::Lines(lines)).await.is_err() {
                break;
            }
        }
    });
    rx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ansi_ve_kontrol_karakterleri_temizlenir() {
        assert_eq!(sanitize_line("\x1b[31mkırmızı\x1b[0m"), "kırmızı");
        assert_eq!(sanitize_line("a\x1b[2J\x1b[1;1Hb"), "ab");
        assert_eq!(sanitize_line("\x1b]0;başlık\x07metin"), "metin");
        assert_eq!(
            sanitize_line("\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\"),
            "link"
        );
        assert_eq!(sanitize_line("x\x1b(By"), "xy");
        assert_eq!(sanitize_line("a\rb\x08c\x7fd\u{9b}e\0f"), "abcdef");
        assert_eq!(sanitize_line("a\tb"), "a b");
        assert_eq!(sanitize_line("a\u{202e}b"), "ab");
        // Yarım kaçış dizisi paniklemez.
        assert_eq!(sanitize_line("son\x1b"), "son");
        assert_eq!(sanitize_line("son\x1b[31"), "son");
    }

    #[test]
    fn satir_bolucu_yarim_satiri_tasir_ve_uzun_satiri_boler() {
        let mut s = LineSplitter::default();
        assert_eq!(s.push(b"bir\nik"), vec!["bir"]);
        assert_eq!(s.push(b"i\n\nuc"), vec!["iki", ""]);
        let lines = s.push(&vec![b'x'; MAX_LINE * 2 + 10]);
        assert!(lines.iter().all(|l| l.len() <= MAX_LINE) && lines.len() == 2);
        // Geçersiz UTF-8 çökertmez.
        assert_eq!(LineSplitter::default().push(b"\xff\xfe\n").len(), 1);
    }

    #[test]
    fn komut_servis_adini_dogrular() {
        let c = journal_command(Some("sshd")).unwrap();
        assert!(c.contains("journalctl -f -n 200 --no-pager -o short-iso -u sshd &"));
        assert!(!journal_command(None).unwrap().contains(" -u "));
        for bad in ["a b", "-x", "a;rm", "a'b", ""] {
            assert!(journal_command(Some(bad)).is_err(), "{bad}");
        }
        // Betik tek tırnak içinde; içinde başka tek tırnak olmamalı.
        assert_eq!(c.matches('\'').count(), 2);
    }

    #[tokio::test]
    async fn demo_akisi_baslar_ve_alici_dusunce_durur() {
        let mut rx = spawn_demo("vds".into(), Some("nginx".into()));
        match rx.recv().await {
            Some(LogMsg::Lines(l)) => {
                assert_eq!(l.len(), 20);
                assert!(l.iter().all(|x| !x.contains('\x1b')));
            }
            other => panic!("{other:?}"),
        }
    }
}
