//! Saf uyarı motoru: IO yok, zaman dışarıdan verilir (test edilebilir).
//!
//! Durum makinesi (anahtar başına): koşul sağlanmıyor -> bekliyor (`for` dolana kadar)
//! -> ateşlendi (tek `Firing`) -> koşul bitince tek `Resolved`.
//! `for` dolmadan koşul biterse hiçbir olay üretilmez.
//!
//! Bilinmeyen metrik (`None`: ayrıştırma hatası, sunucu erişilemez vb.) mevcut durumu
//! DEĞİŞTİRMEZ: bekleyen sayaç başlamaz/sıfırlanmaz, aktif uyarı çözülmez. Aksi halde
//! sunucu erişilemez olunca cpu uyarısı yalnış yere "çözüldü" olurdu.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant, SystemTime};

use crate::config::{self, ConfigError};
use crate::model::ServerSnapshot;

/// Geçmişte tutulan en fazla olay sayısı.
const HISTORY_CAP: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Gt,
    Ge,
    Lt,
    Le,
}

impl Op {
    fn test(self, v: f64, thr: f64) -> bool {
        match self {
            Op::Gt => v > thr,
            Op::Ge => v >= thr,
            Op::Lt => v < thr,
            Op::Le => v <= thr,
        }
    }

    fn sym(self) -> &'static str {
        match self {
            Op::Gt => ">",
            Op::Ge => ">=",
            Op::Lt => "<",
            Op::Le => "<=",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Cpu,
    Mem,
    DiskUsage,
    Swap,
    Load,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Condition {
    Threshold(Metric, Op, f64),
    ServiceDown,
    Unreachable,
}

/// Ayrıştırılmış kural.
#[derive(Debug, Clone, PartialEq)]
pub struct AlertRule {
    /// Orijinal `when` metni (kırpılmış).
    pub when: String,
    pub cond: Condition,
    pub for_duration: Duration,
    /// `"*"` veya tam sunucu adları. Boş liste = hepsi.
    // VARSAYIM: `servers` verilmemişse kural tüm sunucularda geçerlidir; yalnızca
    // "*" ve tam ad desteklenir (gerçek glob yok).
    pub servers: Vec<String>,
}

impl AlertRule {
    pub fn parse(
        when: &str,
        for_duration: Option<Duration>,
        servers: Vec<String>,
    ) -> Result<Self, ConfigError> {
        let when = when.trim();
        Ok(Self {
            cond: parse_condition(when)?,
            when: when.to_string(),
            for_duration: for_duration.unwrap_or_default(),
            servers,
        })
    }

    pub fn from_config(c: &config::AlertRule) -> Result<Self, ConfigError> {
        Self::parse(&c.when, c.for_duration, c.servers.clone())
    }

    fn applies_to(&self, server: &str) -> bool {
        self.servers.is_empty() || self.servers.iter().any(|s| s == "*" || s == server)
    }
}

fn parse_condition(when: &str) -> Result<Condition, ConfigError> {
    let when = when.trim();
    let bad = |why: &str| ConfigError::InvalidAlert(format!("'{when}': {why}"));
    match when {
        "service_down" => return Ok(Condition::ServiceDown),
        "unreachable" => return Ok(Condition::Unreachable),
        _ => {}
    }
    let pos = when
        .find(['<', '>'])
        .ok_or_else(|| bad("işleç (>, >=, <, <=) bulunamadı"))?;
    let metric = match when[..pos].trim() {
        "cpu" => Metric::Cpu,
        "mem" => Metric::Mem,
        "disk_usage" => Metric::DiskUsage,
        "swap" => Metric::Swap,
        "load" => Metric::Load,
        other => return Err(bad(&format!("bilinmeyen metrik '{other}'"))),
    };
    let rest = &when[pos..];
    let (op, num) = match (&rest[..1], rest[1..].strip_prefix('=')) {
        (">", Some(n)) => (Op::Ge, n),
        ("<", Some(n)) => (Op::Le, n),
        (">", None) => (Op::Gt, &rest[1..]),
        _ => (Op::Lt, &rest[1..]),
    };
    let thr: f64 = num
        .trim()
        .parse()
        .ok()
        .filter(|v: &f64| v.is_finite())
        .ok_or_else(|| bad("eşik sayı olmalı"))?;
    Ok(Condition::Threshold(metric, op, thr))
}

/// Config doğrulaması: kurallar ayrıştırılabilir olmalı, adı geçen sunucular var olmalı.
pub fn validate(
    rules: &[config::AlertRule],
    servers: &[config::ServerConfig],
) -> Result<(), ConfigError> {
    for r in rules {
        AlertRule::from_config(r)?;
        for s in &r.servers {
            if s != "*" && !servers.iter().any(|c| &c.name == s) {
                return Err(ConfigError::UnknownAlertServer {
                    rule: r.when.clone(),
                    server: s.clone(),
                });
            }
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertEventKind {
    Firing,
    Resolved,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AlertEvent {
    pub kind: AlertEventKind,
    /// Kararlı kimlik: `sunucu|kural|konu`.
    pub key: String,
    pub server: String,
    pub rule: String,
    pub subject: Option<String>,
    pub message: String,
    /// Koşulun ilk sağlandığı an.
    pub started_at: SystemTime,
    pub at: SystemTime,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveAlert {
    pub server: String,
    pub rule: String,
    pub subject: Option<String>,
    pub message: String,
    pub since: SystemTime,
}

struct Entry {
    server: String,
    rule: String,
    subject: Option<String>,
    since: Instant,
    since_wall: SystemTime,
    firing: bool,
    message: String,
}

/// Bir kuralın bir sunucudaki tek bir konu için gözlemi.
struct Observation {
    subject: Option<String>,
    holds: bool,
    /// Koşul sağlanırken gösterilecek açıklama ("disk kullanımı %93.4 (eşik > 90)").
    detail: String,
}

pub struct AlertEngine {
    rules: Vec<AlertRule>,
    entries: BTreeMap<String, Entry>,
    history: VecDeque<AlertEvent>,
}

impl AlertEngine {
    pub fn new(rules: Vec<AlertRule>) -> Self {
        Self {
            rules,
            entries: BTreeMap::new(),
            history: VecDeque::new(),
        }
    }

    /// Yeni anlık görüntüleri değerlendirir; yalnızca durum değişimlerini döndürür.
    // VARSAYIM: Listeden kaybolan sunucuya ait durum olduğu gibi bırakılır.
    pub fn evaluate(
        &mut self,
        now: Instant,
        wall: SystemTime,
        snapshots: &[ServerSnapshot],
    ) -> Vec<AlertEvent> {
        let mut events = Vec::new();
        for snap in snapshots.iter().filter(|s| s.checked) {
            for i in 0..self.rules.len() {
                if !self.rules[i].applies_to(&snap.name) {
                    continue;
                }
                for obs in observe(&self.rules[i], snap) {
                    let rule = self.rules[i].clone();
                    if let Some(ev) = self.step(&rule, &snap.name, obs, now, wall) {
                        events.push(ev);
                    }
                }
            }
        }
        for ev in &events {
            if self.history.len() == HISTORY_CAP {
                self.history.pop_front();
            }
            self.history.push_back(ev.clone());
        }
        events
    }

    fn step(
        &mut self,
        rule: &AlertRule,
        server: &str,
        obs: Observation,
        now: Instant,
        wall: SystemTime,
    ) -> Option<AlertEvent> {
        let key = format!(
            "{server}|{}|{}",
            rule.when,
            obs.subject.as_deref().unwrap_or("")
        );
        let event = |kind, message, started_at, subject| AlertEvent {
            kind,
            key: key.clone(),
            server: server.to_string(),
            rule: rule.when.clone(),
            subject,
            message,
            started_at,
            at: wall,
        };

        if !obs.holds {
            let e = self.entries.remove(&key)?;
            if !e.firing {
                return None; // `for` dolmadan düzeldi: sessizce sıfırla.
            }
            let msg = format!(
                "{} çözüldü — {} sürdü",
                e.subject.as_deref().unwrap_or(&e.rule),
                fmt_dur(now.saturating_duration_since(e.since))
            );
            return Some(event(
                AlertEventKind::Resolved,
                msg,
                e.since_wall,
                e.subject,
            ));
        }

        let e = self.entries.entry(key.clone()).or_insert_with(|| Entry {
            server: server.to_string(),
            rule: rule.when.clone(),
            subject: obs.subject.clone(),
            since: now,
            since_wall: wall,
            firing: false,
            message: String::new(),
        });
        let held = now.saturating_duration_since(e.since);
        if e.firing || held < rule.for_duration {
            return None;
        }
        e.firing = true;
        e.message = if rule.for_duration.is_zero() {
            obs.detail
        } else {
            format!("{} — {} boyunca", obs.detail, fmt_dur(rule.for_duration))
        };
        Some(event(
            AlertEventKind::Firing,
            e.message.clone(),
            e.since_wall,
            obs.subject,
        ))
    }

    /// Şu an ateşlenmiş (çözülmemiş) uyarılar, başlangıç zamanına göre sıralı.
    pub fn active(&self) -> Vec<ActiveAlert> {
        let mut v: Vec<_> = self.entries.values().filter(|e| e.firing).collect();
        v.sort_by_key(|e| e.since_wall);
        v.into_iter()
            .map(|e| ActiveAlert {
                server: e.server.clone(),
                rule: e.rule.clone(),
                subject: e.subject.clone(),
                message: e.message.clone(),
                since: e.since_wall,
            })
            .collect()
    }

    /// Son olaylar (eskiden yeniye), en fazla 200.
    pub fn history(&self) -> &VecDeque<AlertEvent> {
        &self.history
    }
}

/// Kuralı anlık görüntüye uygular. Boş dönüş = bilinmiyor, durum korunur.
fn observe(rule: &AlertRule, s: &ServerSnapshot) -> Vec<Observation> {
    match rule.cond {
        Condition::Unreachable => vec![Observation {
            subject: None,
            holds: !s.reachable,
            detail: "sunucuya ulaşılamıyor".into(),
        }],
        // Erişilemezken eldeki değerler bayat olabilir: bilinmiyor say.
        _ if !s.reachable => vec![],
        Condition::ServiceDown => s
            .services
            .iter()
            .map(|svc| Observation {
                subject: Some(svc.name.clone()),
                // VARSAYIM: not-found (yüklenmemiş) unit "düşmüş" sayılmaz.
                holds: !svc.is_missing() && !svc.is_running(),
                detail: format!("servis {} çalışmıyor ({})", svc.name, svc.active_state),
            })
            .collect(),
        Condition::Threshold(metric, op, thr) => {
            let Some((v, label)) = metric_value(metric, s) else {
                return vec![];
            };
            let unit = if metric == Metric::Load { "" } else { "%" };
            vec![Observation {
                subject: None,
                holds: op.test(v, thr),
                detail: format!("{label} {unit}{v:.1} (eşik {} {thr})", op.sym()),
            }]
        }
    }
}

fn pct(used: u64, total: u64) -> Option<f64> {
    (total > 0).then(|| used as f64 * 100.0 / total as f64)
}

fn metric_value(m: Metric, s: &ServerSnapshot) -> Option<(f64, String)> {
    match m {
        Metric::Cpu => Some((s.cpu.as_ref()?.total, "CPU kullanımı".into())),
        Metric::Mem => {
            let mem = s.mem?;
            let v = pct(mem.total.saturating_sub(mem.available), mem.total)?;
            Some((v, "RAM kullanımı".into()))
        }
        Metric::Swap => {
            let mem = s.mem?;
            let v = pct(mem.swap_total.saturating_sub(mem.swap_free), mem.swap_total)?;
            Some((v, "swap kullanımı".into()))
        }
        Metric::Load => Some((s.load?.one, "load (1 dk)".into())),
        Metric::DiskUsage => {
            // Tüm diskler içinde en yüksek kullanım; mount mesajda gösterilir.
            s.disks
                .as_ref()?
                .iter()
                .filter_map(|d| Some((pct(d.used, d.total)?, d)))
                .max_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(v, d)| (v, format!("disk kullanımı ({})", d.mount)))
        }
    }
}

fn fmt_dur(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..=59 => format!("{s} sn"),
        60..=3599 => format!("{} dk", s / 60),
        _ => format!("{} sa {} dk", s / 3600, s % 3600 / 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{CpuUsage, DiskUsage, LoadAvg, MemInfo, ServiceStatus};

    fn snap(name: &str) -> ServerSnapshot {
        ServerSnapshot {
            name: name.into(),
            checked: true,
            reachable: true,
            ..Default::default()
        }
    }

    fn with_cpu(name: &str, v: f64) -> ServerSnapshot {
        ServerSnapshot {
            cpu: Some(CpuUsage {
                total: v,
                cores: vec![],
            }),
            ..snap(name)
        }
    }

    fn with_disk(name: &str, pct: u64) -> ServerSnapshot {
        ServerSnapshot {
            disks: Some(vec![DiskUsage {
                filesystem: "/dev/sda1".into(),
                fstype: "ext4".into(),
                mount: "/".into(),
                total: 1000,
                used: pct * 10,
                available: 1000 - pct * 10,
            }]),
            ..snap(name)
        }
    }

    fn svc(name: &str, load: &str, active: &str) -> ServiceStatus {
        ServiceStatus {
            name: name.into(),
            load_state: load.into(),
            active_state: active.into(),
            sub_state: String::new(),
        }
    }

    fn rule(when: &str, for_s: u64, servers: &[&str]) -> AlertRule {
        AlertRule::parse(
            when,
            Some(Duration::from_secs(for_s)),
            servers.iter().map(|s| s.to_string()).collect(),
        )
        .unwrap()
    }

    struct Clock(Instant);
    impl Clock {
        fn at(&self, s: u64) -> (Instant, SystemTime) {
            (
                self.0 + Duration::from_secs(s),
                SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000 + s),
            )
        }
    }

    fn run(e: &mut AlertEngine, c: &Clock, s: u64, snaps: &[ServerSnapshot]) -> Vec<AlertEvent> {
        let (i, w) = c.at(s);
        e.evaluate(i, w, snaps)
    }

    fn clock() -> Clock {
        Clock(Instant::now())
    }

    #[test]
    fn kural_ayristirma() {
        let p = |s: &str| parse_condition(s);
        assert_eq!(
            p("cpu > 85").unwrap(),
            Condition::Threshold(Metric::Cpu, Op::Gt, 85.0)
        );
        assert_eq!(
            p("  mem>=90.5 ").unwrap(),
            Condition::Threshold(Metric::Mem, Op::Ge, 90.5)
        );
        assert_eq!(
            p("disk_usage   <   10").unwrap(),
            Condition::Threshold(Metric::DiskUsage, Op::Lt, 10.0)
        );
        assert_eq!(
            p("load <= 4").unwrap(),
            Condition::Threshold(Metric::Load, Op::Le, 4.0)
        );
        assert_eq!(
            p("swap > 50").unwrap(),
            Condition::Threshold(Metric::Swap, Op::Gt, 50.0)
        );
        assert_eq!(p(" service_down ").unwrap(), Condition::ServiceDown);
        assert_eq!(p("unreachable").unwrap(), Condition::Unreachable);
        for bad in [
            "",
            "cpu",
            "cpu >",
            "cpu > x",
            "foo > 1",
            "cpu = 5",
            "> 5",
            "cpu > NaN",
            "cpu > inf",
        ] {
            assert!(
                matches!(p(bad), Err(ConfigError::InvalidAlert(_))),
                "{bad:?}"
            );
        }
    }

    #[test]
    fn varsayilan_for_sifir_ve_metin_korunur() {
        let r = AlertRule::parse("  cpu > 1 ", None, vec![]).unwrap();
        assert_eq!(r.for_duration, Duration::ZERO);
        assert_eq!(r.when, "cpu > 1");
    }

    #[test]
    fn dogrulama_bilinmeyen_sunucu() {
        let servers = vec![config::ServerConfig {
            name: "a".into(),
            ssh_host: Some("a".into()),
            host: None,
            user: None,
            port: None,
            key: None,
            services: vec![],
            allow_actions: false,
        }];
        let mk = |srv: &str| config::AlertRule {
            when: "cpu > 1".into(),
            for_duration: None,
            servers: vec![srv.into()],
        };
        assert!(validate(&[mk("a"), mk("*")], &servers).is_ok());
        assert!(matches!(
            validate(&[mk("b")], &servers),
            Err(ConfigError::UnknownAlertServer { server, .. }) if server == "b"
        ));
    }

    #[test]
    fn for_suresi_beklenir_tek_kez_ateslenir() {
        let c = clock();
        let mut e = AlertEngine::new(vec![rule("cpu > 85", 60, &["*"])]);
        let hot = [with_cpu("a", 90.0)];
        assert!(run(&mut e, &c, 0, &hot).is_empty());
        assert!(run(&mut e, &c, 59, &hot).is_empty());
        assert!(e.active().is_empty());
        let ev = run(&mut e, &c, 60, &hot);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, AlertEventKind::Firing);
        assert_eq!(ev[0].key, "a|cpu > 85|");
        assert_eq!(ev[0].started_at, c.at(0).1);
        assert!(ev[0].message.contains("%90.0"), "{}", ev[0].message);
        assert!(ev[0].message.contains("1 dk boyunca"), "{}", ev[0].message);
        // Tekrar yok.
        assert!(run(&mut e, &c, 120, &hot).is_empty());
        assert!(run(&mut e, &c, 600, &hot).is_empty());
        assert_eq!(e.active().len(), 1);
        assert_eq!(e.history().len(), 1);
    }

    #[test]
    fn for_sifir_aninda_atesler() {
        let c = clock();
        let mut e = AlertEngine::new(vec![rule("cpu > 85", 0, &["*"])]);
        assert_eq!(run(&mut e, &c, 0, &[with_cpu("a", 99.0)]).len(), 1);
    }

    #[test]
    fn cozuldu_bir_kez_ve_sure_icerir() {
        let c = clock();
        let mut e = AlertEngine::new(vec![rule("cpu > 85", 0, &["*"])]);
        run(&mut e, &c, 0, &[with_cpu("a", 90.0)]);
        let ev = run(&mut e, &c, 300, &[with_cpu("a", 10.0)]);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, AlertEventKind::Resolved);
        assert!(ev[0].message.contains("5 dk"), "{}", ev[0].message);
        assert_eq!(ev[0].started_at, c.at(0).1);
        assert!(run(&mut e, &c, 302, &[with_cpu("a", 10.0)]).is_empty());
        assert!(e.active().is_empty());
        assert_eq!(e.history().len(), 2);
    }

    #[test]
    fn for_dolmadan_duzelirse_olay_yok_ve_sayac_sifirlanir() {
        let c = clock();
        let mut e = AlertEngine::new(vec![rule("cpu > 85", 60, &["*"])]);
        assert!(run(&mut e, &c, 0, &[with_cpu("a", 90.0)]).is_empty());
        assert!(run(&mut e, &c, 30, &[with_cpu("a", 10.0)]).is_empty());
        // Sayaç yeniden başlar: t=40'tan 60 sn sonra, t=60'ta değil.
        assert!(run(&mut e, &c, 40, &[with_cpu("a", 90.0)]).is_empty());
        assert!(run(&mut e, &c, 60, &[with_cpu("a", 90.0)]).is_empty());
        assert_eq!(run(&mut e, &c, 100, &[with_cpu("a", 90.0)]).len(), 1);
        assert_eq!(e.history().len(), 1);
    }

    #[test]
    fn tekrar_ates_ve_cozum_dongusu() {
        let c = clock();
        let mut e = AlertEngine::new(vec![rule("cpu >= 85", 0, &["*"])]);
        run(&mut e, &c, 0, &[with_cpu("a", 85.0)]);
        run(&mut e, &c, 10, &[with_cpu("a", 84.9)]);
        let ev = run(&mut e, &c, 20, &[with_cpu("a", 90.0)]);
        assert_eq!(ev[0].kind, AlertEventKind::Firing);
        assert_eq!(ev[0].started_at, c.at(20).1);
    }

    #[test]
    fn servis_basina_anahtar_ve_eksik_servis_yok_sayilir() {
        let c = clock();
        let mut e = AlertEngine::new(vec![rule("service_down", 0, &["*"])]);
        let mut s = snap("a");
        s.services = vec![
            svc("nginx", "loaded", "active"),
            svc("pg", "loaded", "failed"),
            svc("redis", "loaded", "inactive"),
            svc("ghost", "not-found", "inactive"),
        ];
        let ev = run(&mut e, &c, 0, &[s.clone()]);
        let keys: Vec<_> = ev.iter().map(|x| x.key.as_str()).collect();
        assert_eq!(keys, ["a|service_down|pg", "a|service_down|redis"]);
        assert_eq!(ev[0].subject.as_deref(), Some("pg"));
        // pg düzelir, redis düşük kalır.
        s.services[1] = svc("pg", "loaded", "active");
        let ev = run(&mut e, &c, 10, &[s]);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].kind, AlertEventKind::Resolved);
        assert_eq!(ev[0].key, "a|service_down|pg");
        let act = e.active();
        assert_eq!(act.len(), 1);
        assert_eq!(act[0].subject.as_deref(), Some("redis"));
    }

    #[test]
    fn bilinmeyen_metrik_durumu_korur() {
        let c = clock();
        let mut e = AlertEngine::new(vec![rule("cpu > 85", 30, &["*"])]);
        // Bekleyen sayaç: metrik kaybolsa da sıfırlanmaz.
        run(&mut e, &c, 0, &[with_cpu("a", 90.0)]);
        assert!(run(&mut e, &c, 20, &[snap("a")]).is_empty());
        assert_eq!(run(&mut e, &c, 30, &[with_cpu("a", 90.0)]).len(), 1);
        // Aktif uyarı: metrik yokken çözülmez.
        assert!(run(&mut e, &c, 40, &[snap("a")]).is_empty());
        assert_eq!(e.active().len(), 1);
        // Hiç başlamamış: bilinmeyen metrik beklemeyi başlatmaz.
        let mut e2 = AlertEngine::new(vec![rule("cpu > 85", 0, &["*"])]);
        assert!(run(&mut e2, &c, 0, &[snap("a")]).is_empty());
        assert!(e2.entries.is_empty());
    }

    #[test]
    fn erisilemez_sunucu_metrik_alarmini_cozmez() {
        let c = clock();
        let mut e = AlertEngine::new(vec![
            rule("cpu > 85", 0, &["*"]),
            rule("unreachable", 0, &["*"]),
        ]);
        run(&mut e, &c, 0, &[with_cpu("a", 90.0)]);
        // Bayat cpu değeri kalsa bile erişilemezken cpu kuralı bilinmiyor sayılır.
        let mut down = with_cpu("a", 10.0);
        down.reachable = false;
        let ev = run(&mut e, &c, 10, &[down.clone()]);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].rule, "unreachable");
        assert_eq!(ev[0].kind, AlertEventKind::Firing);
        assert!(run(&mut e, &c, 20, &[down]).is_empty());
        assert_eq!(e.active().len(), 2);
        let ev = run(&mut e, &c, 30, &[with_cpu("a", 10.0)]);
        assert_eq!(ev.len(), 2); // unreachable + cpu çözüldü
        assert!(ev.iter().all(|x| x.kind == AlertEventKind::Resolved));
    }

    #[test]
    fn kontrol_edilmemis_sunucu_yok_sayilir() {
        let c = clock();
        let mut e = AlertEngine::new(vec![rule("unreachable", 0, &["*"])]);
        let s = ServerSnapshot {
            name: "a".into(),
            ..Default::default()
        };
        assert!(run(&mut e, &c, 0, &[s]).is_empty());
    }

    #[test]
    fn sunucu_filtresi() {
        let c = clock();
        let mut e = AlertEngine::new(vec![rule("cpu > 85", 0, &["a"])]);
        let ev = run(&mut e, &c, 0, &[with_cpu("a", 90.0), with_cpu("b", 90.0)]);
        assert_eq!(ev.len(), 1);
        assert_eq!(ev[0].server, "a");
        let mut e = AlertEngine::new(vec![rule("cpu > 85", 0, &["*"])]);
        assert_eq!(
            run(&mut e, &c, 0, &[with_cpu("a", 90.0), with_cpu("b", 90.0)]).len(),
            2
        );
        let mut e = AlertEngine::new(vec![rule("cpu > 85", 0, &[])]);
        assert_eq!(run(&mut e, &c, 0, &[with_cpu("b", 90.0)]).len(), 1);
    }

    #[test]
    fn kaybolan_sunucu_durumu_korunur() {
        let c = clock();
        let mut e = AlertEngine::new(vec![rule("cpu > 85", 0, &["*"])]);
        run(&mut e, &c, 0, &[with_cpu("a", 90.0)]);
        assert!(run(&mut e, &c, 10, &[]).is_empty());
        assert_eq!(e.active().len(), 1);
    }

    #[test]
    fn disk_en_yuksek_kullanim_ve_mesaj() {
        let c = clock();
        let mut e = AlertEngine::new(vec![rule("disk_usage > 90", 0, &["*"])]);
        let mut s = with_disk("a", 50);
        s.disks.as_mut().unwrap().push(DiskUsage {
            filesystem: "/dev/sdb".into(),
            fstype: "ext4".into(),
            mount: "/data".into(),
            total: 1000,
            used: 934,
            available: 66,
        });
        let ev = run(&mut e, &c, 0, &[s]);
        assert_eq!(ev.len(), 1);
        assert!(ev[0].message.contains("%93.4"), "{}", ev[0].message);
        assert!(ev[0].message.contains("/data"));
        assert!(ev[0].message.contains("eşik > 90"));
    }

    #[test]
    fn mem_swap_load() {
        let c = clock();
        let mut e = AlertEngine::new(vec![
            rule("mem > 90", 0, &["*"]),
            rule("swap > 50", 0, &["*"]),
            rule("load > 4", 0, &["*"]),
        ]);
        let mut s = snap("a");
        s.mem = Some(MemInfo {
            total: 1000,
            available: 50,
            swap_total: 100,
            swap_free: 40,
        });
        s.load = Some(LoadAvg {
            one: 4.5,
            five: 0.0,
            fifteen: 0.0,
        });
        assert_eq!(run(&mut e, &c, 0, &[s.clone()]).len(), 3);
        // swap_total = 0 -> bilinmiyor, durum korunur.
        s.mem = Some(MemInfo {
            total: 1000,
            available: 50,
            swap_total: 0,
            swap_free: 0,
        });
        assert!(run(&mut e, &c, 5, &[s]).is_empty());
        assert_eq!(e.active().len(), 3);
    }

    #[test]
    fn gecmis_sinirli() {
        let c = clock();
        let mut e = AlertEngine::new(vec![rule("cpu > 85", 0, &["*"])]);
        for i in 0..(HISTORY_CAP as u64 + 50) {
            let v = if i % 2 == 0 { 90.0 } else { 10.0 };
            run(&mut e, &c, i, &[with_cpu("a", v)]);
        }
        assert_eq!(e.history().len(), HISTORY_CAP);
        // En eski olaylar atıldı: son olay çözüldü olmalı.
        assert_eq!(e.history().back().unwrap().kind, AlertEventKind::Resolved);
    }
}
