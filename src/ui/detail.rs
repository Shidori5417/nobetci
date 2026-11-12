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

/// Çekirdek numarasının hane sayısı (en az 2): etiketler hizalı kalır.
fn core_digits(n: usize) -> usize {
    n.saturating_sub(1).to_string().len().max(2)
}

/// `n` çekirdeği `w`×`h` alana yerleştirir; `(sütun, gösterilen çekirdek)` döner.
/// Sığmazsa son satır "+N çekirdek" göstergesine ayrılır (gösterilen sayı buna göre düşer).
fn core_layout(n: usize, w: u16, h: u16) -> (usize, usize) {
    // Hücre: "c" + numara + boşluk + gauge (15) + ayırıcı boşluk.
    let cell_w = core_digits(n) + 18;
    let (rows, max_cols) = (h as usize, (w as usize / cell_w).max(1));
    if n == 0 || rows == 0 {
        return (1, 0);
    }
    if n <= rows * max_cols {
        // Gereğinden fazla sütun açılmaz.
        return (n.div_ceil(rows), n);
    }
    // Sığmıyor: bir satır gösterge için ayrılır.
    (max_cols, (rows - 1) * max_cols)
}

fn draw_cores(f: &mut Frame, area: Rect, st: &ServerState) {
    let block = Block::bordered();
    let inner = block.inner(area);
    let mut title = " Çekirdekler ".to_string();
    let lines: Vec<Line> = match &st.latest.cpu {
        Some(c) if !c.cores.is_empty() => {
            title = format!(" Çekirdekler (toplam {:.0}%) ", c.total);
            let n = c.cores.len();
            let d = core_digits(n);
            let (cols, shown) = core_layout(n, inner.width, inner.height);
            let mut lines: Vec<Line> = (0..shown)
                .step_by(cols)
                .map(|row| {
                    let mut spans = Vec::new();
                    for i in row..(row + cols).min(shown) {
                        spans.push(Span::raw(format!("c{i:<d$} ")));
                        spans.push(gauge(Some(c.cores[i])));
                        spans.push(Span::raw(" "));
                    }
                    Line::from(spans)
                })
                .collect();
            if shown < n {
                lines.push(Line::styled(
                    format!("+{} çekirdek", n - shown),
                    Style::new().fg(Color::DarkGray),
                ));
            }
            lines
        }
        _ => vec![Line::styled("veri yok", Style::new().fg(Color::DarkGray))],
    };
    f.render_widget(Paragraph::new(lines).block(block.title(title)), area);
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
    fn cekirdek_izgarasi_sigar_ya_da_gosterge_ayirir() {
        // 8 çekirdek, 46 geniş (2 sütun), 7 satır: 2 sütun x 4 satır yeter.
        assert_eq!(core_layout(8, 46, 7), (2, 8));
        // Tek sütun, 7 satır: 6 gösterilir + gösterge satırı.
        assert_eq!(core_layout(8, 30, 7), (1, 6));
        // 128 çekirdek her zaman sınırı aşmaz.
        let (cols, shown) = core_layout(128, 100, 10);
        assert!(cols * 9 >= shown && shown <= 45);
        assert_eq!(core_layout(8, 0, 5).0, 1);
        assert_eq!(core_layout(8, 10, 0), (1, 0));
        assert_eq!(core_layout(0, 10, 5), (1, 0));
        for (n, w, h) in [(1, 1, 1), (64, 200, 3), (5, 19, 1), (128, 20, 2)] {
            let (c, s) = core_layout(n, w, h);
            assert!(c >= 1 && s <= n && s <= c * (h as usize));
        }
    }

    fn many_cores(n: usize) -> App {
        let mut snap = demo::snapshots(1).remove(0);
        if let Some(c) = &mut snap.cpu {
            c.cores = (0..n).map(|i| (i % 101) as f64).collect();
        }
        App::new(vec![snap])
    }

    fn screen(term: &Terminal<TestBackend>) -> String {
        term.backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect()
    }

    #[test]
    fn cok_cekirdekte_her_boyutta_panic_olmaz_ve_gosterge_cikar() {
        for n in [8, 64, 128] {
            let mut app = many_cores(n);
            app.detail = true;
            for (w, h) in [(80, 24), (40, 10), (1, 1)] {
                let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
                term.draw(|f| draw(f, &app)).unwrap();
                if (w, h) == (80, 24) {
                    let text = screen(&term);
                    // 8 çekirdek 80x24te sığabilir; 64 ve 128 için gösterge şart.
                    assert!(n == 8 || text.contains("çekirdek"), "n={n}");
                    assert!(text.contains("c0"));
                }
            }
        }
    }

    #[test]
    fn genis_ekranda_8_cekirdek_cok_sutunda_hepsi_gorunur() {
        let mut app = many_cores(8);
        app.detail = true;
        let mut term = Terminal::new(TestBackend::new(160, 45)).unwrap();
        term.draw(|f| draw(f, &app)).unwrap();
        let text = screen(&term);
        assert!(text.contains("c7") && !text.contains("+"), "{text}");
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
