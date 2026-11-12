//! Uyarılar ekranı: aktif uyarılar ve geçmiş (yeniden eskiye).

use std::time::SystemTime;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};

use super::App;
use super::overview::draw_help;
use crate::alert::AlertEventKind;
use crate::timefmt;

/// Kaydırılan toplam satır sayısı (başlıklar ve boş satırlar dahil).
pub fn line_count(app: &App) -> usize {
    app.alerts.active.len().max(1) + app.alerts.history.len().max(1) + 3
}

fn lines(app: &App) -> Vec<Line<'static>> {
    let dim = Style::new().fg(Color::DarkGray);
    let bold = Style::new().add_modifier(Modifier::BOLD);
    let now = SystemTime::now();
    let mut out = vec![Line::styled(
        format!("Aktif uyarılar ({})", app.alerts.active.len()),
        bold,
    )];
    if app.alerts.active.is_empty() {
        out.push(Line::styled("  Aktif uyarı yok", dim));
    }
    for a in &app.alerts.active {
        let held = now.duration_since(a.since).unwrap_or_default();
        let subject = a
            .subject
            .as_deref()
            .map_or(String::new(), |s| format!(" [{s}]"));
        out.push(Line::styled(
            format!(
                "● {}  {}{}  {} ({})  {}",
                a.server,
                a.rule,
                subject,
                timefmt::local(a.since),
                timefmt::duration(held),
                a.message
            ),
            Style::new().fg(Color::Red),
        ));
    }
    out.push(Line::raw(""));
    out.push(Line::styled(
        format!("Geçmiş ({})", app.alerts.history.len()),
        bold,
    ));
    if app.alerts.history.is_empty() {
        out.push(Line::styled("  Kayıt yok", dim));
    }
    for e in &app.alerts.history {
        let (mark, color) = match e.kind {
            AlertEventKind::Firing => ("▲", Color::Red),
            AlertEventKind::Resolved => ("✓", Color::Green),
        };
        out.push(Line::styled(
            format!(
                "{mark} {}  {}  {}  {}",
                timefmt::local(e.at),
                e.server,
                e.rule,
                e.message
            ),
            Style::new().fg(color),
        ));
    }
    out
}

pub fn draw(f: &mut Frame, app: &App) {
    let [main, footer] =
        Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(f.area());
    let scroll = app.alert_scroll.min(u16::MAX as usize) as u16;
    f.render_widget(
        Paragraph::new(lines(app))
            .scroll((scroll, 0))
            .block(Block::bordered().title(" Uyarılar ")),
        main,
    );
    f.render_widget(
        Paragraph::new(" j/k: kaydır   Esc: geri   ?: yardım")
            .style(Style::new().fg(Color::DarkGray)),
        footer,
    );
    if app.show_help {
        draw_help(f);
    }
}
