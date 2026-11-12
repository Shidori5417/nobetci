//! Log görüntüleyici: sınırlı tampon, takip modu, süzme ve kaydırma.
//!
//! Kaydırma "alttan uzaklık" (`back`) ile tutulur: 0 = takip (otomatik kaydırma).
//! Takip dışındayken yeni satır gelince `back` artar, böylece görünüm yerinde kalır.

use std::cell::Cell;
use std::collections::VecDeque;

use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};
use tokio::sync::mpsc;

use crate::logs::LogMsg;

/// Tamponda tutulan en fazla satır; dolunca en eskiler atılır.
pub const MAX_LINES: usize = 5000;

/// Tuşun sonucu: görüntüleyici açık kalır, kapanır ya da yardım istenir.
#[derive(Debug, PartialEq, Eq)]
pub enum LogKey {
    Stay,
    Close,
    Help,
}

pub struct LogView {
    title: String,
    lines: VecDeque<String>,
    rx: mpsc::Receiver<LogMsg>,
    /// Akış bitti (kanal kapandı); tekrar tekrar `recv` edilmesin.
    ended: bool,
    error: Option<String>,
    filter: String,
    editing: bool,
    back: usize,
    /// Son çizimdeki içerik yüksekliği (PgUp/PgDn adımı).
    page: Cell<usize>,
}

impl LogView {
    pub fn new(title: String, rx: mpsc::Receiver<LogMsg>) -> Self {
        Self {
            title,
            lines: VecDeque::new(),
            rx,
            ended: false,
            error: None,
            filter: String::new(),
            editing: false,
            back: 0,
            page: Cell::new(10),
        }
    }

    /// Sonraki akış iletisi; akış bittiyse sonsuza dek bekler (select dalı için).
    pub async fn recv(&mut self) -> Option<LogMsg> {
        if self.ended {
            return std::future::pending().await;
        }
        let m = self.rx.recv().await;
        self.ended = m.is_none();
        m
    }

    pub fn on_msg(&mut self, msg: Option<LogMsg>) {
        match msg {
            Some(LogMsg::Lines(v)) => self.push_lines(v),
            Some(LogMsg::Error(e)) => self.error = Some(e),
            None => {
                self.error.get_or_insert_with(|| "akış sona erdi".into());
            }
        }
    }

    pub fn push_lines(&mut self, new: Vec<String>) {
        let needle = self.filter.to_lowercase();
        for l in new {
            if self.lines.len() == MAX_LINES {
                self.lines.pop_front();
            }
            // Takipte değilsek görünüm kaymasın.
            if self.back > 0 && matches(&l, &needle) {
                self.back += 1;
            }
            self.lines.push_back(l);
        }
        self.back = self.back.min(self.count().saturating_sub(1));
    }

    fn count(&self) -> usize {
        if self.filter.is_empty() {
            return self.lines.len();
        }
        let needle = self.filter.to_lowercase();
        self.lines.iter().filter(|l| matches(l, &needle)).count()
    }

    pub fn on_key(&mut self, code: KeyCode) -> LogKey {
        if self.editing {
            match code {
                KeyCode::Esc => {
                    self.filter.clear();
                    self.editing = false;
                }
                KeyCode::Enter => self.editing = false,
                KeyCode::Backspace => {
                    self.filter.pop();
                }
                KeyCode::Char(c) => self.filter.push(c),
                _ => {}
            }
            self.back = 0;
            return LogKey::Stay;
        }
        let max = self.count().saturating_sub(1);
        let page = self.page.get().saturating_sub(1).max(1);
        match code {
            KeyCode::Esc | KeyCode::Char('q') => return LogKey::Close,
            KeyCode::Char('?') => return LogKey::Help,
            KeyCode::Char('/') => self.editing = true,
            KeyCode::Char('k') | KeyCode::Up => self.back = (self.back + 1).min(max),
            KeyCode::Char('j') | KeyCode::Down => self.back = self.back.saturating_sub(1),
            KeyCode::PageUp => self.back = (self.back + page).min(max),
            KeyCode::PageDown => self.back = self.back.saturating_sub(page),
            // İlk satır en üstte görünecek biçimde.
            KeyCode::Char('g') | KeyCode::Home => {
                self.back = self.count().saturating_sub(self.page.get().max(1));
            }
            KeyCode::Char('G') | KeyCode::End => self.back = 0,
            _ => {}
        }
        LogKey::Stay
    }

    pub fn draw(&self, f: &mut Frame) {
        let [body, footer] =
            Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(f.area());
        let needle = self.filter.to_lowercase();
        let shown: Vec<&String> = self.lines.iter().filter(|l| matches(l, &needle)).collect();
        let mode = if self.back == 0 {
            "takip".to_string()
        } else {
            format!("duraklatıldı, {} satır geride", self.back)
        };
        let block = Block::bordered().title(format!(" Loglar: {} [{mode}] ", self.title));
        let h = block.inner(body).height as usize;
        self.page.set(h);
        let end = shown.len().saturating_sub(self.back);
        let start = end.saturating_sub(h);
        let lines: Vec<Line> = if shown.is_empty() {
            let msg = if self.lines.is_empty() {
                "bağlanıyor / veri bekleniyor..."
            } else {
                "süzgece uyan satır yok"
            };
            vec![Line::styled(msg, Style::new().fg(Color::DarkGray))]
        } else {
            shown[start..end]
                .iter()
                .map(|l| Line::raw(l.as_str()))
                .collect()
        };
        f.render_widget(Paragraph::new(lines).block(block), body);

        let footer_line = if self.editing {
            Paragraph::new(format!(
                " /{}█   (Enter: uygula, Esc: temizle)",
                self.filter
            ))
        } else if let Some(e) = &self.error {
            Paragraph::new(format!(" ✗ {e}   (Esc: kapat)")).style(Style::new().fg(Color::Red))
        } else if !self.filter.is_empty() {
            Paragraph::new(format!(
                " süzgeç: {:?} ({}/{} satır)   /: değiştir   Esc: kapat",
                self.filter,
                shown.len(),
                self.lines.len()
            ))
        } else {
            Paragraph::new(
                " j/k: kaydır   PgUp/PgDn   g/G: baş/son (G: takip)   /: süz   Esc: kapat",
            )
            .style(Style::new().fg(Color::DarkGray))
        };
        f.render_widget(footer_line, footer);
    }
}

/// Büyük/küçük harf duyarsız alt dize eşleşmesi (`needle` küçük harfli verilir).
fn matches(line: &str, needle: &str) -> bool {
    needle.is_empty() || line.to_lowercase().contains(needle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn view() -> LogView {
        let (_tx, rx) = mpsc::channel(1);
        LogView::new("t".into(), rx)
    }

    fn lines(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("satır {i} Hata")).collect()
    }

    #[test]
    fn tampon_sinirli_en_eskiyi_atar() {
        let mut v = view();
        v.push_lines(lines(MAX_LINES + 10));
        assert_eq!(v.lines.len(), MAX_LINES);
        assert_eq!(v.lines.front().unwrap(), "satır 10 Hata");
    }

    #[test]
    fn kaydirma_takip_ve_g() {
        let mut v = view();
        v.push_lines(lines(100));
        v.page.set(20);
        v.on_key(KeyCode::Char('k'));
        v.on_key(KeyCode::PageUp);
        assert_eq!(v.back, 1 + 19);
        // Takipte değilken yeni satır görünümü kaydırmaz.
        v.push_lines(lines(3));
        assert_eq!(v.back, 23);
        v.on_key(KeyCode::Char('g'));
        assert_eq!(v.back, 103 - 20);
        v.on_key(KeyCode::Char('G'));
        assert_eq!(v.back, 0);
        v.on_key(KeyCode::Char('j'));
        assert_eq!(v.back, 0);
        // Üstte sınırlanır.
        for _ in 0..500 {
            v.on_key(KeyCode::PageUp);
        }
        assert_eq!(v.back, 102);
    }

    #[test]
    fn suzgec_buyuk_kucuk_harf_duyarsiz_ve_esc_temizler() {
        let mut v = view();
        v.push_lines(vec!["a ERROR b".into(), "tamam".into(), "error 2".into()]);
        v.on_key(KeyCode::Char('/'));
        for c in "Error".chars() {
            v.on_key(KeyCode::Char(c));
        }
        assert_eq!(v.count(), 2);
        v.on_key(KeyCode::Enter);
        assert!(!v.editing && v.count() == 2);
        // Düzenleme dışında Esc görüntüleyiciyi kapatır.
        assert_eq!(v.on_key(KeyCode::Esc), LogKey::Close);
        v.on_key(KeyCode::Char('/'));
        // Düzenlemede `q`/`?` yazıdır; Esc süzgeci temizler, kapatmaz.
        v.on_key(KeyCode::Char('q'));
        assert_eq!(v.on_key(KeyCode::Esc), LogKey::Stay);
        assert!(v.filter.is_empty() && !v.editing && v.count() == 3);
    }

    #[test]
    fn hata_ve_akis_sonu_gosterilir() {
        let mut v = view();
        v.on_msg(Some(LogMsg::Error("bağlanılamadı".into())));
        assert_eq!(v.error.as_deref(), Some("bağlanılamadı"));
        let mut term = Terminal::new(TestBackend::new(80, 10)).unwrap();
        term.draw(|f| v.draw(f)).unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("bağlanılamadı"));
    }

    #[test]
    fn her_boyutta_panic_olmaz() {
        let mut v = view();
        for (w, h) in [(80, 24), (1, 1), (0, 0), (5, 2), (200, 3)] {
            let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
            term.draw(|f| v.draw(f)).unwrap();
            v.push_lines(lines(50));
            v.on_key(KeyCode::Char('k'));
            term.draw(|f| v.draw(f)).unwrap();
        }
    }
}
