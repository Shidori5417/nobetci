//! Tek SSH exec çağrısında toplanan çıktının oluşturulması ve bölünmesi.

use std::collections::HashMap;

use super::{
    ParseError, cpu::parse_stat, disk::parse_df, load::parse_loadavg, mem::parse_meminfo,
    net::parse_net_dev, service::parse_services, uptime::parse_uptime,
};
use crate::model::{CpuStat, DiskUsage, LoadAvg, MemInfo, NetDev, ServiceStatus};
use std::time::Duration;

const MARKER_PREFIX: &str = "@@NOBETCI:";
const MARKER_SUFFIX: &str = "@@";

/// Servis adı güvenli mi? `[A-Za-z0-9@._:-]+`, tire ile başlayamaz (seçenek sanılmasın).
pub fn valid_service_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'@' | b'.' | b'_' | b':' | b'-'))
}

/// Uzak sunucuda çalışacak komutu üretir.
///
/// Çıktı `sh -c '...'` ile sarılır, böylece kullanıcının girişi kabuğu (fish vb.)
/// ne olursa olsun POSIX sh çalışır. Betik içinde tek tırnak yoktur; servis adları
/// doğrulanıp (`[A-Za-z0-9@._:-]+`) geçersizler sessizce atlanır, bu yüzden ayrıca
/// kabuk tırnaklaması gerekmez. Her komutun hatası `/dev/null`'a gider ve bir
/// bölümün başarısızlığı diğerlerini etkilemez.
pub fn build_command(services: &[String]) -> String {
    // VARSAYIM: LC_ALL=C ile df başlığı ve sayı biçimi yerelden bağımsız olur.
    let mut s = String::from("LC_ALL=C; export LC_ALL; ");
    let mut section = |name: &str, cmd: &str| {
        s.push_str(&format!(
            "echo {MARKER_PREFIX}{name}{MARKER_SUFFIX}; {cmd} 2>/dev/null; "
        ));
    };
    section("stat", "cat /proc/stat");
    section("meminfo", "cat /proc/meminfo");
    section("loadavg", "cat /proc/loadavg");
    section("uptime", "cat /proc/uptime");
    section("df", "df -P -B1 -T");
    section("netdev", "cat /proc/net/dev");

    let mut names: Vec<&str> = Vec::new();
    for n in services
        .iter()
        .map(String::as_str)
        .filter(|n| valid_service_name(n))
    {
        if !names.contains(&n) {
            names.push(n);
        }
    }
    if names.is_empty() {
        s.push_str(&format!("echo {MARKER_PREFIX}services{MARKER_SUFFIX}; "));
    } else {
        section(
            "services",
            &format!(
                "systemctl show {} --property=Id,LoadState,ActiveState,SubState",
                names.join(" ")
            ),
        );
    }
    s.push_str(&format!("echo {MARKER_PREFIX}end{MARKER_SUFFIX}"));
    debug_assert!(!s.contains('\''));
    format!("sh -c '{s}'")
}

/// `@@NOBETCI:ad@@` satırından adı çıkarır.
fn marker_name(line: &str) -> Option<&str> {
    let name = line
        .trim()
        .strip_prefix(MARKER_PREFIX)?
        .strip_suffix(MARKER_SUFFIX)?;
    (!name.is_empty()).then_some(name)
}

/// Çıktıyı bölümlere ayırır: bölüm adı -> gövde.
///
/// VARSAYIM: yalnızca ardından başka bir ayraç (veya `end`) gelen bölümler
/// döner; `end` görülmeden kesilen son bölüm yarım olabileceği için atılır.
/// `end` sonrası her şey yok sayılır. Aynı ad tekrar ederse sonuncusu geçerlidir.
pub fn split_sections(output: &str) -> HashMap<&str, &str> {
    let mut map = HashMap::new();
    let mut current: Option<(&str, usize)> = None;
    let mut pos = 0usize;
    for raw in output.split_inclusive('\n') {
        let start = pos;
        pos += raw.len();
        let Some(name) = marker_name(raw) else {
            continue;
        };
        if let Some((prev, body_start)) = current.take() {
            map.insert(prev, &output[body_start..start]);
        }
        if name == "end" {
            break;
        }
        current = Some((name, pos));
    }
    map
}

/// Her metriğin bağımsız sonucu; bir bölümün bozukluğu diğerlerini etkilemez.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedBatch {
    pub stat: Result<CpuStat, ParseError>,
    pub mem: Result<MemInfo, ParseError>,
    pub load: Result<LoadAvg, ParseError>,
    pub uptime: Result<Duration, ParseError>,
    pub disks: Result<Vec<DiskUsage>, ParseError>,
    pub net: Result<Vec<NetDev>, ParseError>,
    pub services: Result<Vec<ServiceStatus>, ParseError>,
}

/// Toplu çıktıyı ayrıştırır. Eksik bölüm `MissingSection` hatası olur.
pub fn parse_batch(output: &str) -> ParsedBatch {
    let sections = split_sections(output);
    let get = |name: &'static str| {
        sections
            .get(name)
            .copied()
            .ok_or(ParseError::MissingSection(name))
    };
    ParsedBatch {
        stat: get("stat").and_then(parse_stat),
        mem: get("meminfo").and_then(parse_meminfo),
        load: get("loadavg").and_then(parse_loadavg),
        uptime: get("uptime").and_then(parse_uptime),
        disks: get("df").and_then(parse_df),
        net: get("netdev").and_then(parse_net_dev),
        services: get("services").map(parse_services),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: &str = include_str!("../../tests/fixtures/batch_output.txt");
    const BROKEN: &str = include_str!("../../tests/fixtures/batch_broken.txt");

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn command_contains_markers_and_commands() {
        let c = build_command(&names(&["nginx", "ssh"]));
        for m in [
            "stat", "meminfo", "loadavg", "uptime", "df", "netdev", "services", "end",
        ] {
            assert!(c.contains(&format!("@@NOBETCI:{m}@@")), "{m}");
        }
        assert!(c.contains("cat /proc/stat"));
        assert!(c.contains("df -P -B1 -T"));
        assert!(
            c.contains("systemctl show nginx ssh --property=Id,LoadState,ActiveState,SubState")
        );
        assert!(c.starts_with("sh -c '") && c.ends_with('\''));
        // dış tırnaklar dışında tek tırnak yok
        assert!(!c[7..c.len() - 1].contains('\''));
    }

    #[test]
    fn command_rejects_unsafe_names() {
        let c = build_command(&names(&[
            "ok.service",
            "a; rm -rf /",
            "$(id)",
            "x'y",
            "`id`",
            "-h",
            "",
            "a b",
            "ok.service",
            "getty@tty1.service",
            "a\nb",
        ]));
        assert!(c.contains("systemctl show ok.service getty@tty1.service --property"));
        assert!(c.contains("--property=Id,LoadState,ActiveState,SubState"));
        for bad in ["rm", "$(", "`", "-h ", "x'y"] {
            assert!(!c.contains(bad), "{bad}");
        }
        assert!(!c[7..c.len() - 1].contains('\''));
    }

    #[test]
    fn command_without_services_skips_systemctl() {
        for list in [vec![], names(&["bad name", "$x"])] {
            let c = build_command(&list);
            assert!(!c.contains("systemctl"));
            assert!(c.contains("@@NOBETCI:services@@"));
        }
    }

    #[test]
    fn splits_fixture() {
        let s = split_sections(OK);
        assert_eq!(s.len(), 7);
        assert!(s["stat"].starts_with("cpu  "));
        assert_eq!(s["loadavg"].trim(), "0.42 0.37 0.30 2/412 1893054");
        assert!(s["services"].contains("nginx.service"));
        assert!(!s.contains_key("end"));
    }

    #[test]
    fn split_edge_cases() {
        assert!(split_sections("").is_empty());
        assert!(split_sections("junk\nno markers\n").is_empty());
        // end yok: son bölüm atılır
        let s = split_sections("@@NOBETCI:a@@\n1\n@@NOBETCI:b@@\n2\n");
        assert_eq!(s.len(), 1);
        assert_eq!(s["a"], "1\n");
        // CRLF, boş bölüm, end sonrası çöp
        let s = split_sections(
            "@@NOBETCI:a@@\r\n@@NOBETCI:b@@\r\nx\r\n@@NOBETCI:end@@\r\n@@NOBETCI:c@@\nz\n",
        );
        assert_eq!(s["a"], "");
        assert_eq!(s["b"], "x\r\n");
        assert!(!s.contains_key("c"));
        // yarım ayraç yok sayılır
        let s = split_sections("@@NOBETCI:a@@\n@@NOBETCI:b\n@@NOBETCI:end@@\n");
        assert_eq!(s["a"], "@@NOBETCI:b\n");
        // çok baytlı karakterler
        let s = split_sections("@@NOBETCI:a@@\nçğş\n@@NOBETCI:end@@");
        assert_eq!(s["a"], "çğş\n");
    }

    #[test]
    fn parses_full_batch() {
        let p = parse_batch(OK);
        assert_eq!(p.stat.unwrap().cores.len(), 4);
        assert!(p.mem.unwrap().total > 0);
        assert_eq!(p.load.unwrap().one, 0.42);
        assert_eq!(p.uptime.unwrap().as_secs(), 1234567);
        assert_eq!(p.disks.unwrap().len(), 3);
        assert_eq!(p.net.unwrap().len(), 3);
        assert_eq!(p.services.unwrap().len(), 3);
    }

    #[test]
    fn broken_sections_are_independent() {
        let p = parse_batch(BROKEN);
        assert!(p.stat.is_ok());
        assert!(matches!(
            p.mem,
            Err(ParseError::Malformed { .. }) | Err(ParseError::MissingField(_))
        ));
        assert!(p.load.is_ok());
        assert_eq!(p.uptime, Err(ParseError::MissingSection("uptime")));
        assert!(p.disks.is_err());
        assert!(p.net.is_ok());
        assert_eq!(p.services.unwrap().len(), 1);
    }

    #[test]
    fn garbage_and_empty_batch() {
        for input in ["", "garbage", "@@NOBETCI:end@@"] {
            let p = parse_batch(input);
            assert_eq!(p.stat, Err(ParseError::MissingSection("stat")));
            assert!(p.mem.is_err() && p.load.is_err() && p.uptime.is_err());
            assert!(p.disks.is_err() && p.net.is_err() && p.services.is_err());
        }
    }

    #[test]
    fn empty_sections_present() {
        let p = parse_batch("@@NOBETCI:stat@@\n@@NOBETCI:services@@\n@@NOBETCI:end@@\n");
        assert_eq!(p.stat, Err(ParseError::Empty));
        assert_eq!(p.services, Ok(vec![]));
    }
}
