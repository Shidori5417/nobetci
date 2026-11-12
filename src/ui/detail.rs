//! Sunucu detay ekranı: çekirdek çubukları, sparkline'lar, disk ve servis tabloları.

use std::collections::VecDeque;
use std::time::Instant;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Sparkline, Table};

use super::App;
use super::overview::{draw_help, format_uptime, gauge, health, health_icon};
use crate::model::{DiskUsage, ServiceStatus};
use crate::state::ServerState;

/// "1.5G" biçiminde okunabilir boyut.
pub fn format_bytes(b: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b}B")
    } else {
        format!("{v:.1}{}", UNITS[i])
    }
}

/// Bayt/sn hızını "1.5M/s" biçiminde yazar.
pub fn format_rate(bps: f64) -> String {
    format!("{}/s", format_bytes(bps.max(0.0) as u64))
}

/// Geçmişin sığan son `width` noktası (Sparkline baştan okuduğu için kuyruk alınır).
fn tail(buf: &VecDeque<f64>, width: u16) -> Vec<u64> {
    let skip = buf.len().saturating_sub(width as usize);
    buf.iter().skip(skip).map(|v| v.max(0.0) as u64).collect()
}

fn disk_percent(d: &DiskUsage) -> Option<f64> {
    let denom = d.used + d.available;
    (denom > 0).then(|| 100.0 * d.used as f64 / denom as f64)
}

/// Servis durumu: metin ve renk. Bulunamayan unit çökme değil, sarı "bulunamadı".
fn service_state(s: &ServiceStatus) -> (String, Color) {
    if s.is_missing() {
        ("bulunamadı".into(), Color::Yellow)
    } else if s.is_running() {
        (format!("{}/{}", s.active_state, s.sub_state), Color::Green)
    } else {
        (format!("{}/{}", s.active_state, s.sub_state), Color::Red)
    }
}

fn retry_text(retry_at: Option<Instant>) -> String {
    match retry_at {
        Some(t) => match t.checked_duration_since(Instant::now()) {
            Some(d) => format!("yeniden deneme: {} sn sonra", d.as_secs() + 1),
            None => "yeniden deneniyor...".into(),
        },
        None => String::new(),
    }
}

pub fn draw(f: &mut Frame, app: &App) {
    let [head, body, footer] = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let Some(st) = app.states.get(app.selected) else {
        return;
    };

    draw_header(f, head, st);
    let [top, bottom] =
        Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);
    let [cores, spark] =
        Layout::horizontal([Constraint::Percentage(40), Constraint::Percentage(60)]).areas(top);
    let [disks, services] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(bottom);
    draw_cores(f, cores, st);
    draw_sparklines(f, spark, st);
    draw_disks(f, disks, st);
    draw_services(f, services, st);

    f.render_widget(
        Paragraph::new(" Esc: geri   j/k: sunucu değiştir   ?: yardım   q: çıkış")
            .style(Style::new().fg(Color::DarkGray)),
        footer,
    );
    if app.show_help {
        draw_help(f);
    }
}

fn draw_header(f: &mut Frame, area: Rect, st: &ServerState) {
    let s = &st.latest;
    let dash = || "-".to_string();
    let load = s.load.map_or_else(dash, |l| {
        format!("{:.2} {:.2} {:.2}", l.one, l.five, l.fifteen)
    });
    let up = s.uptime.map_or_else(dash, format_uptime);
    let line1 = format!(
        " {} {}   load {load}   uptime {up}",
        health_icon(health(s)),
        s.name
    );
    let line2 = if !s.checked {
        Line::styled(" henüz denenmedi", Style::new().fg(Color::DarkGray))
    } else if s.reachable {
        Line::styled(" ● erişilebilir", Style::new().fg(Color::Green))
    } else {
        let err = s.error.as_deref().unwrap_or("erişilemiyor");
        Line::styled(
            format!(" ✗ {err}   {}", retry_text(s.retry_at)),
            Style::new().fg(Color::Red),
        )
    };
    f.render_widget(
        Paragraph::new(vec![Line::raw(line1), line2])
            .block(Block::bordered().title(" Sunucu detayı ")),
        area,
    );
}

fn draw_cores(f: &mut Frame, area: Rect, st: &ServerState) {
    let lines: Vec<Line> = match &st.latest.cpu {
        Some(c) if !c.cores.is_empty() => c
            .cores
            .iter()
            .enumerate()
            .map(|(i, v)| Line::from(vec![Span::raw(format!("c{i:<2} ")), gauge(Some(*v))]))
            .collect(),
        _ => vec![Line::styled("veri yok", Style::new().fg(Color::DarkGray))],
    };
    let total = st.latest.cpu.as_ref().map(|c| c.total);
    let title = match total {
        Some(t) => format!(" Çekirdekler (toplam {t:.0}%) "),
        None => " Çekirdekler ".into(),
    };
    // Paragraph taşan satırları keser; çok çekirdekli sunucuda ilk satırlar görünür.
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title)),
        area,
    );
}

fn draw_sparklines(f: &mut Frame, area: Rect, st: &ServerState) {
    let areas: [Rect; 4] = Layout::vertical([Constraint::Fill(1); 4]).areas(area);
    let pct = |b: &VecDeque<f64>| b.back().map_or("-".into(), |v| format!("{v:.0}%"));
    let rate = |b: &VecDeque<f64>| b.back().map_or("-".into(), |v| format_rate(*v));
    let series = [
        (
            format!(" CPU {} ", pct(&st.cpu)),
            &st.cpu,
            Some(100),
            Color::Cyan,
        ),
        (
            format!(" RAM {} ", pct(&st.ram)),
            &st.ram,
            Some(100),
            Color::Magenta,
        ),
        (
            format!(" Ağ ↓ {} ", rate(&st.net_rx)),
            &st.net_rx,
            None,
            Color::Green,
        ),
        (
            format!(" Ağ ↑ {} ", rate(&st.net_tx)),
            &st.net_tx,
            None,
            Color::Yellow,
        ),
    ];
    for (a, (title, buf, max, color)) in areas.into_iter().zip(series) {
        let block = Block::new().borders(Borders::TOP).title(title);
        let inner = block.inner(a);
        f.render_widget(block, a);
        let mut sp = Sparkline::default()
            .data(tail(buf, inner.width))
            .style(Style::new().fg(color));
        if let Some(m) = max {
            sp = sp.max(m);
        }
        f.render_widget(sp, inner);
    }
}

fn draw_disks(f: &mut Frame, area: Rect, st: &ServerState) {
    let block = Block::bordered().title(" Diskler ");
    let Some(disks) = &st.latest.disks else {
        f.render_widget(
            Paragraph::new("veri yok")
                .style(Style::new().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    };
    let rows = disks.iter().map(|d| {
        Row::new([
            Cell::from(d.mount.clone()),
            Cell::from(d.fstype.clone()),
            Cell::from(format!(
                "{} / {}",
                format_bytes(d.used),
                format_bytes(d.used + d.available)
            )),
            Cell::from(gauge(disk_percent(d))),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Min(6),
            Constraint::Length(6),
            Constraint::Length(13),
            Constraint::Length(15),
        ],
    )
    .header(
        Row::new(["Bağ", "Tür", "Kullanım", "%"]).style(Style::new().add_modifier(Modifier::BOLD)),
    )
    .block(block);
    f.render_widget(table, area);
}

fn draw_services(f: &mut Frame, area: Rect, st: &ServerState) {
    let block = Block::bordered().title(" Servisler ");
    if st.latest.services.is_empty() {
        f.render_widget(
            Paragraph::new("izlenen servis yok")
                .style(Style::new().fg(Color::DarkGray))
                .block(block),
            area,
        );
        return;
    }
    let rows = st.latest.services.iter().map(|s| {
        let (text, color) = service_state(s);
        Row::new([
            Cell::from(s.name.clone()),
            Cell::from(Span::styled(text, Style::new().fg(color))),
        ])
    });
    let table = Table::new(rows, [Constraint::Min(8), Constraint::Length(18)])
        .header(Row::new(["Ad", "Durum"]).style(Style::new().add_modifier(Modifier::BOLD)))
        .block(block);
    f.render_widget(table, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;
    use ratatui::{Terminal, backend::TestBackend};

    #[test]
    fn boyut_bicimi() {
        assert_eq!(format_bytes(512), "512B");
        assert_eq!(format_bytes(1536), "1.5K");
        assert_eq!(format_bytes(3 << 30), "3.0G");
        assert_eq!(format_rate(2048.0), "2.0K/s");
    }

    #[test]
    fn kuyruk_son_noktalari_alir() {
        let b: VecDeque<f64> = (0..10).map(f64::from).collect();
        assert_eq!(tail(&b, 3), vec![7, 8, 9]);
        assert_eq!(tail(&b, 0), Vec::<u64>::new());
        assert_eq!(tail(&b, 50).len(), 10);
    }

    /// Her boyutta, her sunucu için (erişilemez dahil) panic olmadan çizilmeli.
    #[test]
    fn kucuk_ve_buyuk_boyutta_panic_olmaz() {
        let mut app = App::new(demo::snapshots(0));
        for t in 1..5 {
            app.set_servers(&demo::snapshots(t));
        }
        app.detail = true;
        for (w, h) in [
            (80, 24),
            (160, 45),
            (40, 10),
            (20, 5),
            (1, 1),
            (0, 0),
            (200, 3),
        ] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            for sel in 0..4 {
                app.selected = sel;
                app.show_help = sel == 1;
                term.draw(|f| draw(f, &app)).unwrap();
            }
        }
    }
}
