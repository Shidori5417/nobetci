//! `systemctl show --property=Id,LoadState,ActiveState,SubState` ayrıştırma.

use crate::model::ServiceStatus;

/// Bir `key=value` bloğunu biriktirir.
#[derive(Default)]
struct Block {
    id: Option<String>,
    load: Option<String>,
    active: Option<String>,
    sub: Option<String>,
}

impl Block {
    fn finish(self, out: &mut Vec<ServiceStatus>) {
        // Id ve ActiveState olmayan blok anlamsızdır, atlanır.
        if let (Some(id), Some(active_state)) = (self.id, self.active) {
            let name = id.strip_suffix(".service").unwrap_or(&id).to_string();
            if name.is_empty() {
                return;
            }
            out.push(ServiceStatus {
                name,
                // VARSAYIM: LoadState yoksa "unknown" denir (eski systemd / bozuk çıktı).
                load_state: self.load.unwrap_or_else(|| "unknown".to_string()),
                active_state,
                // VARSAYIM: SubState yoksa "unknown" denir.
                sub_state: self.sub.unwrap_or_else(|| "unknown".to_string()),
            });
        }
    }
}

/// Boş satırla ayrılmış blokları okur. Bozuk/eksik bloklar atlanır; boş girdi
/// boş liste verir (servis izlenmiyor olabilir).
pub fn parse_services(input: &str) -> Vec<ServiceStatus> {
    let mut out = Vec::new();
    let mut cur = Block::default();
    for line in input.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            std::mem::take(&mut cur).finish(&mut out);
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k {
            "Id" => cur.id = Some(v.to_string()),
            "LoadState" => cur.load = Some(v.to_string()),
            "ActiveState" => cur.active = Some(v.to_string()),
            "SubState" => cur.sub = Some(v.to_string()),
            _ => {}
        }
    }
    cur.finish(&mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = include_str!("../../tests/fixtures/systemctl_show.txt");

    #[test]
    fn parses_fixture() {
        let v = parse_services(S);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].name, "nginx");
        assert_eq!(v[0].active_state, "active");
        assert_eq!(v[0].sub_state, "running");
        assert!(v[0].is_running());
        assert_eq!(v[1].name, "ssh");
        assert!(!v[1].is_running());
        assert_eq!(v[2].active_state, "inactive");
        assert_eq!(v[2].sub_state, "dead");
    }

    #[test]
    fn not_found_unit_is_missing_not_down_by_load_state() {
        let v = parse_services(S);
        assert_eq!(v[0].load_state, "loaded");
        assert!(!v[0].is_missing());
        assert_eq!(v[2].name, "missing");
        assert_eq!(v[2].load_state, "not-found");
        assert!(v[2].is_missing());
        assert!(!v[2].is_running());
    }

    #[test]
    fn missing_load_state_is_unknown_not_missing() {
        let v = parse_services("Id=a.service\nActiveState=active\nSubState=running\n");
        assert_eq!(v[0].load_state, "unknown");
        assert!(!v[0].is_missing());
    }

    #[test]
    fn empty_and_garbage() {
        assert!(parse_services("").is_empty());
        assert!(parse_services("\n\n\n").is_empty());
        assert!(parse_services("garbage\nno equals here\n").is_empty());
    }

    #[test]
    fn truncated_block() {
        // ActiveState'ten önce kesilmiş blok atlanır
        assert!(parse_services("Id=a.service\n").is_empty());
        // SubState eksikse unknown
        let v = parse_services("Id=a.service\nActiveState=active");
        assert_eq!(v[0].sub_state, "unknown");
    }

    #[test]
    fn no_trailing_blank_and_crlf() {
        let v = parse_services("Id=a.service\r\nActiveState=active\r\nSubState=running\r\n");
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].sub_state, "running");
    }

    #[test]
    fn non_service_units_keep_suffix() {
        let v = parse_services("Id=backup.timer\nActiveState=active\nSubState=waiting\n");
        assert_eq!(v[0].name, "backup.timer");
        let v = parse_services("Id=getty@tty1.service\nActiveState=active\nSubState=running\n");
        assert_eq!(v[0].name, "getty@tty1");
    }

    #[test]
    fn value_containing_equals_and_extra_blanks() {
        let v = parse_services(
            "\n\nId=a.service\nDescription=x=y\nActiveState=active\nSubState=running\n\n\n\nId=b.service\nActiveState=failed\nSubState=failed\n",
        );
        assert_eq!(v.len(), 2);
    }
}
