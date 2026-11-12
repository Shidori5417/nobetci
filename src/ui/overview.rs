//! Genel bakış ekranı.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Constraint, Flex, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Cell, Clear, Paragraph, Row, Table, TableState};

use super::App;
use crate::model::{Health, ServerSnapshot};

/// Kritik eşik (%).
const WARN: f64 = 90.0;
const CRIT: f64 = 95.0;

pub fn mem_percent(s: &ServerSnapshot) -> Option<f64> {
    let m = s.mem.as_ref().filter(|m| m.total > 0)?;
    Some(100.0 * (m.total - m.available.min(m.total)) as f64 / m.total as f64)
}

/// En dolu diskin kullanım yüzdesi.
pub fn max_disk_percent(s: &ServerSnapshot) -> Option<f64> {
    s.disks
        .as_ref()?
        .iter()
        .filter(|d| d.used + d.available > 0)
        .map(|d| 100.0 * d.used as f64 / (d.used + d.available) as f64)
        .reduce(f64::max)
}

/// Çalışmayan ama var olan (çökmüş/durmuş) servis sayısı.
pub fn down_services(s: &ServerSnapshot) -> usize {
    s.services
        .iter()
        .filter(|x| !x.is_running() && !x.is_missing())
        .count()
}

/// Unit'i hiç bulunmayan servis sayısı.
pub fn missing_services(s: &ServerSnapshot) -> usize {
    s.services.iter().filter(|x| x.is_missing()).count()
}

/// Snapshot'tan sağlık durumu türetir.
// VARSAYIM: hiç denenmemiş sunucu Unknown; erişilemez veya servis düşmüş veya
// herhangi bir metrik >=95 ise Critical; CPU/RAM/disk >90 veya bulunamayan
// servis (yanlış yazılmış ad olabilir, çökme değil) varsa Warning.
pub fn health(s: &ServerSnapshot) -> Health {
    if !s.checked {
        return Health::Unknown;
    }
    if !s.reachable || down_services(s) > 0 {
        return Health::Critical;
    }
    let vals = [
        s.cpu.as_ref().map(|c| c.total),
        mem_percent(s),
        max_disk_percent(s),
    ];
    let max = vals.into_iter().flatten().fold(0.0, f64::max);
    if max >= CRIT {
        Health::Critical
    } else if max > WARN || missing_services(s) > 0 {
        Health::Warning
    } else {
        Health::Ok
    }
}

pub(super) fn health_icon(h: Health) -> &'static str {
    match h {
        Health::Ok => "🟢",
        Health::Warning => "🟡",
        Health::Critical => "🔴",
        Health::Unknown => "⚪",
    }
}

/// "████░░░░ 42%" biçiminde metin gauge'u; bilinmiyorsa "-".
pub(super) fn gauge(pct: Option<f64>) -> Span<'static> {
    const W: usize = 10;
    let Some(p) = pct else {
        return Span::styled("-", Style::new().fg(Color::DarkGray));
    };
    let filled = ((p.clamp(0.0, 100.0) / 100.0) * W as f64).round() as usize;
    let color = if p >= CRIT {
        Color::Red
    } else if p > WARN {
        Color::Yellow
    } else {
        Color::Green
    };
    Span::styled(
        format!(
            "{}{} {:>3.0}%",
            "█".repeat(filled),
            "░".repeat(W - filled),
            p
        ),
        Style::new().fg(color),
    )
}

pub fn format_uptime(d: Duration) -> String {
    let s = d.as_secs();
    let (days, hours, mins) = (s / 86400, s % 86400 / 3600, s % 3600 / 60);
    if days > 0 {
        format!("{days}g {hours}s")
    } else if hours > 0 {
        format!("{hours}s {mins}d")
    } else {
        format!("{mins}d")
    }
}

pub fn draw(f: &mut Frame, app: &App, table_state: &mut TableState) {
    let [main, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());

    let header = Row::new(["", "Ad", "CPU", "RAM", "Disk", "Load", "Uptime", "Servis"])
        .style(Style::new().add_modifier(Modifier::BOLD));
    let rows = app.states.iter().map(|st| {
        let s = &st.latest;
        let dash = || Span::styled("-", Style::new().fg(Color::DarkGray));
        let down = down_services(s);
        let down_cell = if !s.checked || !s.reachable {
            Span::raw("-")
        } else if down > 0 {
            Span::styled(format!("{down} düştü"), Style::new().fg(Color::Red))
        } else if missing_services(s) > 0 {
            Span::styled(
                format!("{} yok", missing_services(s)),
                Style::new().fg(Color::Yellow),
            )
        } else {
            Span::styled("0", Style::new().fg(Color::Green))
        };
        Row::new([
            Cell::from(health_icon(health(s))),
            Cell::from(s.name.clone()),
            Cell::from(gauge(s.cpu.as_ref().map(|c| c.total))),
            Cell::from(gauge(mem_percent(s))),
            Cell::from(gauge(max_disk_percent(s))),
            Cell::from(
                s.load
                    .map_or(dash(), |l| Span::raw(format!("{:.2}", l.one))),
            ),
            Cell::from(s.uptime.map_or(dash(), |u| Span::raw(format_uptime(u)))),
            Cell::from(down_cell),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(3),
            Constraint::Min(10),
            Constraint::Length(15),
            Constraint::Length(15),
            Constraint::Length(15),
            Constraint::Length(6),
            Constraint::Length(8),
            Constraint::Length(9),
        ],
    )
    .header(header)
    .block(Block::bordered().title(" Nöbetçi "))
    .row_highlight_style(Style::new().add_modifier(Modifier::REVERSED));

    table_state.select((!app.states.is_empty()).then_some(app.selected));
    f.render_stateful_widget(table, main, table_state);

    // Seçili sunucunun son hatası varsa durum satırında gösterilir.
    let err = app
        .states
        .get(app.selected)
        .and_then(|st| st.latest.error.as_deref());
    let footer_line = match err {
        Some(e) => Paragraph::new(format!(" ✗ {e}")).style(Style::new().fg(Color::Red)),
        None => Paragraph::new(" j/k: seç   Enter: detay   ?: yardım   q: çıkış")
            .style(Style::new().fg(Color::DarkGray)),
    };
    f.render_widget(footer_line, footer);

    if app.show_help {
        draw_help(f);
    }
}

const HELP: &[(&str, &str)] = &[
    ("↑ ↓ / j k", "Sunucu seç (servis/log listesinde gez)"),
    ("Enter", "Sunucu detayı"),
    ("Esc", "Geri / kapat"),
    ("s", "Detayda servis listesine odaklan"),
    ("r", "Seçili servisi yeniden başlat (onay ister)"),
    ("l", "Loglar (seçili servis ya da tüm günlük)"),
    ("/", "Log araması (Enter uygula, Esc temizle)"),
    ("n / N", "Sonraki / önceki eşleşme (uçlarda sarar)"),
    ("f", "Yalnızca eşleşen satırları göster / tümü"),
    ("g / G", "Log başı / sonu (G: canlı takip)"),
    ("PgUp PgDn", "Log sayfa kaydır"),
    ("?", "Yardım"),
    ("q", "Çıkış"),
];

pub(super) fn draw_help(f: &mut Frame) {
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(k, d)| {
            Line::from(vec![
                Span::styled(format!(" {k:<10}"), Style::new().fg(Color::Cyan)),
                Span::raw(*d),
            ])
        })
        .collect();
    let area = centered(f.area(), 60, lines.len() as u16 + 2);
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Kısayollar ")),
        area,
    );
}

pub(super) fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let [v] = Layout::vertical([Constraint::Length(h.min(area.height))])
        .flex(Flex::Center)
        .areas(area);
    let [r] = Layout::horizontal([Constraint::Length(w.min(area.width))])
        .flex(Flex::Center)
        .areas(v);
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    #[test]
    fn demo_saglik_durumlari() {
        let h: Vec<_> = demo::snapshots(0).iter().map(health).collect();
        assert_eq!(h[0], Health::Ok);
        assert_eq!(h[1], Health::Warning); // yüksek disk + bulunamayan servis
        assert_eq!(h[2], Health::Critical); // düşen servis
        assert_eq!(h[3], Health::Critical); // erişilemez
        assert_eq!(health(&ServerSnapshot::default()), Health::Unknown);
    }

    #[test]
    fn bulunamayan_servis_cokme_sayilmaz() {
        let s = &demo::snapshots(0)[1];
        assert_eq!((down_services(s), missing_services(s)), (0, 1));
    }

    #[test]
    fn uptime_bicimi() {
        assert_eq!(format_uptime(Duration::from_secs(90_000)), "1g 1s");
        assert_eq!(format_uptime(Duration::from_secs(300)), "5d");
    }
}
