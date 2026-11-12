//! Log görüntüleyici: sınırlı tampon, takip modu, arama (n/N), süzme ve kaydırma.
//!
//! Kaydırma "alttan uzaklık" (`back`) ile tutulur: 0 = takip (otomatik kaydırma).
//! Takip dışındayken yeni satır gelince `back` artar, böylece görünüm yerinde kalır.

use std::cell::Cell;
use std::collections::VecDeque;

use crossterm::event::KeyCode;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
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
    /// Arama terimi (büyük/küçük harf duyarsız alt dize).
    term: String,
    /// `true` ise yalnızca eşleşen satırlar görünür (`f`).
    only: bool,
    /// `n`/`N` hedefi: `lines` içindeki mutlak indeks.
    cursor: Option<usize>,
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
            term: String::new(),
            only: false,
            cursor: None,
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
        let needle = fold_str(&self.term);
        for l in new {
            if self.lines.len() == MAX_LINES {
                self.lines.pop_front();
                // İndeksler kaydı; en eski satırsa hedef kaybolur.
                self.cursor = self.cursor.and_then(|c| c.checked_sub(1));
            }
            // Takipte değilsek görünüm kaymasın.
            if self.back > 0 && self.visible(&l, &needle) {
                self.back += 1;
            }
            self.lines.push_back(l);
        }
        self.back = self.back.min(self.count().saturating_sub(1));
    }

    /// Satır şu an listede görünür mü? (`f` kapalıysa hepsi.)
    fn visible(&self, line: &str, needle: &[char]) -> bool {
        !self.only || !find_spans(line, needle).is_empty()
    }

    fn count(&self) -> usize {
        let needle = fold_str(&self.term);
        self.lines
            .iter()
            .filter(|l| self.visible(l, &needle))
            .count()
    }

    /// Eşleşen satırların mutlak indeksleri.
    fn match_indices(&self) -> Vec<usize> {
        let needle = fold_str(&self.term);
        self.lines
            .iter()
            .enumerate()
            .filter(|(_, l)| !find_spans(l, &needle).is_empty())
            .map(|(i, _)| i)
            .collect()
    }

    /// Hedef satırı görünür alanın ortasına getirir ve takipten çıkar.
    fn reveal(&mut self, idx: usize) {
        let needle = fold_str(&self.term);
        let pos = if self.only {
            self.lines
                .iter()
                .take(idx)
                .filter(|l| !find_spans(l, &needle).is_empty())
                .count()
        } else {
            idx
        };
        let len = self.count();
        let end = (pos + 1 + self.page.get() / 2).min(len);
        self.back = len - end;
        // Hedef alttaysa bile takipten çıkılır (en az 1 satır geride).
        if self.back == 0 && len > 1 {
            self.back = 1;
        }
    }

    /// `n` (ileri) / `N` (geri): bir sonraki eşleşmeye atlar; uçlarda başa/sona sarar.
    // VARSAYIM: uçlarda sarma yapılır; hedef yoksa ileri ilk, geri son eşleşmeden başlar.
    fn jump(&mut self, forward: bool) {
        let m = self.match_indices();
        let next = match (self.cursor, forward) {
            _ if m.is_empty() => None,
            (Some(c), true) => m.iter().find(|&&i| i > c).or(m.first()),
            (Some(c), false) => m.iter().rev().find(|&&i| i < c).or(m.last()),
            (None, true) => m.first(),
            (None, false) => m.last(),
        }
        .copied();
        self.cursor = next;
        if let Some(i) = next {
            self.reveal(i);
        }
    }

    pub fn on_key(&mut self, code: KeyCode) -> LogKey {
        if self.editing {
            match code {
                KeyCode::Esc => {
                    self.term.clear();
                    self.editing = false;
                }
                KeyCode::Enter => {
                    self.editing = false;
                    // VARSAYIM: arama sonrası en yeni (en alttaki) eşleşmeye atlanır.
                    self.cursor = self.match_indices().last().copied();
                    if let Some(i) = self.cursor {
                        self.reveal(i);
                    }
                    return LogKey::Stay;
                }
                KeyCode::Backspace => {
                    self.term.pop();
                }
                KeyCode::Char(c) => self.term.push(c),
                _ => {}
            }
            self.cursor = None;
            self.back = 0;
            return LogKey::Stay;
        }
        let max = self.count().saturating_sub(1);
        let page = self.page.get().saturating_sub(1).max(1);
        match code {
            KeyCode::Esc | KeyCode::Char('q') => return LogKey::Close,
            KeyCode::Char('?') => return LogKey::Help,
            KeyCode::Char('/') => self.editing = true,
            KeyCode::Char('n') => self.jump(true),
            KeyCode::Char('N') => self.jump(false),
            KeyCode::Char('f') => {
                self.only = !self.only;
                match self.cursor {
                    Some(i) => self.reveal(i),
                    None => self.back = 0,
                }
            }
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
        let needle = fold_str(&self.term);
        let shown: Vec<(usize, &String)> = self
            .lines
            .iter()
            .enumerate()
            .filter(|(_, l)| self.visible(l, &needle))
            .collect();
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
                "süzgece uyan satır yok (f: süzgeci kapat)"
            };
            vec![Line::styled(msg, Style::new().fg(Color::DarkGray))]
        } else {
            shown[start..end]
                .iter()
                .map(|&(i, l)| render_line(l, &needle, self.cursor == Some(i)))
                .collect()
        };
        f.render_widget(Paragraph::new(lines).block(block), body);

        let footer_line = if self.editing {
            Paragraph::new(format!(" /{}█   (Enter: ara, Esc: temizle)", self.term))
        } else if let Some(e) = &self.error {
            Paragraph::new(format!(" ✗ {e}   (Esc: kapat)")).style(Style::new().fg(Color::Red))
        } else if self.only && !self.term.is_empty() {
            Paragraph::new(format!(
                " süzgeç: {:?} ({}/{} satır)   f: süzgeci kapat   /: ara   Esc: kapat",
                self.term,
                shown.len(),
                self.lines.len()
            ))
        } else if !self.term.is_empty() {
            let m = self.match_indices();
            let status = match self.cursor.and_then(|c| m.iter().position(|&i| i == c)) {
                Some(p) => format!("{}/{} eşleşme", p + 1, m.len()),
                None if m.is_empty() => "eşleşme yok".to_string(),
                None => format!("{} eşleşme", m.len()),
            };
            Paragraph::new(format!(
                " arama: {:?} {status}   n/N: sonraki/önceki   f: süz   /: değiştir   Esc: kapat",
                self.term
            ))
        } else {
            Paragraph::new(
                " j/k: kaydır   PgUp/PgDn   g/G: baş/son (G: takip)   /: ara   Esc: kapat",
            )
            .style(Style::new().fg(Color::DarkGray))
        };
        f.render_widget(footer_line, footer);
    }
}

/// Karakter başına büyük/küçük harf katlama. `to_lowercase` bazı karakterlerde birden
/// fazla karakter üretir ('İ' -> "i̇"); bayt kaydırması oluşmasın diye her karakter tek
/// karaktere indirilir (ilk küçük harf karşılığı). Böylece 'İ' hem 'i' hem 'İ' ile eşleşir.
fn fold(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

fn fold_str(s: &str) -> Vec<char> {
    s.chars().map(fold).collect()
}

/// `line` içindeki çakışmayan eşleşmelerin bayt aralıkları (özgün metne göre, karakter sınırlarında).
fn find_spans(line: &str, needle: &[char]) -> Vec<(usize, usize)> {
    let n = needle.len();
    if n == 0 {
        return Vec::new();
    }
    let chars: Vec<(usize, char)> = line.char_indices().map(|(i, c)| (i, fold(c))).collect();
    let (mut spans, mut i) = (Vec::new(), 0);
    while i + n <= chars.len() {
        if chars[i..i + n]
            .iter()
            .map(|x| x.1)
            .eq(needle.iter().copied())
        {
            let end = chars.get(i + n).map_or(line.len(), |x| x.0);
            spans.push((chars[i].0, end));
            i += n;
        } else {
            i += 1;
        }
    }
    spans
}

/// Satırı eşleşen / eşleşmeyen parçalara böler (`true` = eşleşme).
fn split_spans<'a>(line: &'a str, spans: &[(usize, usize)]) -> Vec<(&'a str, bool)> {
    let (mut out, mut pos) = (Vec::new(), 0);
    for &(s, e) in spans {
        if s > pos {
            out.push((&line[pos..s], false));
        }
        out.push((&line[s..e], true));
        pos = e;
    }
    if pos < line.len() {
        out.push((&line[pos..], false));
    }
    out
}

/// Tek satırı çizime hazırlar. Arama varsa 2 sütunluk sol boşluk ayrılır;
/// geçerli n/N hedefi `▶` ile işaretlenir, eşleşmeler siyah-sarı vurgulanır.
fn render_line<'a>(line: &'a str, needle: &[char], current: bool) -> Line<'a> {
    if needle.is_empty() {
        return Line::raw(line);
    }
    let hit = if current {
        Style::new()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(Color::Black).bg(Color::Yellow)
    };
    let gutter = if current {
        Span::styled("▶ ", Style::new().fg(Color::Yellow))
    } else {
        Span::raw("  ")
    };
    let mut spans = vec![gutter];
    for (t, h) in split_spans(line, &find_spans(line, needle)) {
        spans.push(if h {
            Span::styled(t, hit)
        } else {
            Span::raw(t)
        });
    }
    Line::from(spans)
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

    fn type_term(v: &mut LogView, t: &str) {
        v.on_key(KeyCode::Char('/'));
        for c in t.chars() {
            v.on_key(KeyCode::Char(c));
        }
        v.on_key(KeyCode::Enter);
    }

    #[test]
    fn arama_tum_satirlari_tutar_f_suzer_esc_temizler() {
        let mut v = view();
        v.push_lines(vec!["a ERROR b".into(), "tamam".into(), "error 2".into()]);
        type_term(&mut v, "Error");
        assert!(!v.editing && v.count() == 3 && v.match_indices() == vec![0, 2]);
        v.on_key(KeyCode::Char('f'));
        assert_eq!(v.count(), 2);
        v.on_key(KeyCode::Char('f'));
        assert_eq!(v.count(), 3);
        // Düzenleme dışında Esc görüntüleyiciyi kapatır.
        assert_eq!(v.on_key(KeyCode::Esc), LogKey::Close);
        v.on_key(KeyCode::Char('/'));
        // Düzenlemede `q`/`n`/`f` yazıdır; Esc terimi temizler, kapatmaz.
        v.on_key(KeyCode::Char('q'));
        assert_eq!(v.on_key(KeyCode::Esc), LogKey::Stay);
        assert!(v.term.is_empty() && !v.editing && v.count() == 3);
    }

    #[test]
    fn n_ve_buyuk_n_gezer_sarar_ve_takipten_cikar() {
        let mut v = view();
        v.page.set(10);
        let mut l = lines(100);
        for i in [10, 50, 90] {
            l[i] = format!("satır {i} WARN");
        }
        v.push_lines(l);
        type_term(&mut v, "warn");
        // Enter en yeni eşleşmeye atlar ve takipten çıkar.
        assert_eq!(v.cursor, Some(90));
        assert!(v.back > 0);
        v.on_key(KeyCode::Char('n'));
        assert_eq!(v.cursor, Some(10), "sarar");
        v.on_key(KeyCode::Char('n'));
        assert_eq!(v.cursor, Some(50));
        v.on_key(KeyCode::Char('N'));
        v.on_key(KeyCode::Char('N'));
        assert_eq!(v.cursor, Some(90), "geri sarar");
        // Hedef pencerede görünür.
        let end = 100 - v.back;
        assert!(end > 90 && end - 10 <= 90);
        // Süzgeçte de hedef görünür ve eşleşmeyen yok.
        v.on_key(KeyCode::Char('f'));
        assert_eq!(v.count(), 3);
        v.on_key(KeyCode::Char('N'));
        assert_eq!(v.cursor, Some(50));
        // Eşleşme yoksa n bir şey yapmaz.
        type_term(&mut v, "yokyok");
        v.on_key(KeyCode::Char('n'));
        assert_eq!(v.cursor, None);
    }

    #[test]
    fn durum_satiri_ve_vurgu_cizilir() {
        let mut v = view();
        v.push_lines(vec!["a WARN b".into(), "x".into(), "warn".into()]);
        type_term(&mut v, "WARN");
        v.on_key(KeyCode::Char('N'));
        let mut term = Terminal::new(TestBackend::new(100, 8)).unwrap();
        term.draw(|f| v.draw(f)).unwrap();
        let buf = term.backend().buffer();
        let text: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(text.contains("arama: \"WARN\" 1/2 eşleşme"), "{text}");
        assert!(text.contains('▶'));
        assert!(buf.content().iter().any(|c| c.bg == Color::Yellow));
        v.on_key(KeyCode::Char('f'));
        term.draw(|f| v.draw(f)).unwrap();
        let text: String = term
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|c| c.symbol())
            .collect();
        assert!(text.contains("(2/3 satır)"), "{text}");
    }

    #[test]
    fn vurgu_bolme_turkce_ve_cok_baytli() {
        fn sp<'a>(l: &'a str, n: &str) -> Vec<(&'a str, bool)> {
            split_spans(l, &find_spans(l, &fold_str(n)))
        }
        assert_eq!(
            sp("Hata: HATA hata", "hata"),
            vec![
                ("Hata", true),
                (": ", false),
                ("HATA", true),
                (" ", false),
                ("hata", true)
            ]
        );
        // 'İ' küçük harfte 2 karakter olur; bayt kaydırması olmamalı.
        assert_eq!(
            sp("İstanbul İSTEK", "i"),
            vec![
                ("İ", true),
                ("stanbul ", false),
                ("İ", true),
                ("STEK", false)
            ]
        );
        assert_eq!(
            sp("ÇĞ çğ", "çğ"),
            vec![("ÇĞ", true), (" ", false), ("çğ", true)]
        );
        assert_eq!(
            sp("şş😀ş", "😀"),
            vec![("şş", false), ("😀", true), ("ş", false)]
        );
        // Eşleşme yok / boş terim / terim satırdan uzun: panik yok.
        assert_eq!(sp("abc", "x"), vec![("abc", false)]);
        assert!(find_spans("abc", &[]).is_empty());
        assert!(find_spans("a", &fold_str("abc")).is_empty());
        assert_eq!(sp("", "a"), vec![]);
        // Parçalar birleşince özgün satırı verir.
        let l = "ıİiI öÖüÜ ğĞ";
        for n in ["i", "ö", "ğ", "ıi"] {
            let joined: String = sp(l, n).iter().map(|x| x.0).collect();
            assert_eq!(joined, l);
        }
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
