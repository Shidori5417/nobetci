//! ratatui ekranları.

mod detail;
mod overview;

use color_eyre::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use tokio::sync::watch;

use crate::model::ServerSnapshot;
use crate::state::ServerState;

/// UI durumu.
pub struct App {
    /// Sunucu başına son snapshot + geçmiş (UI sahibidir).
    pub states: Vec<ServerState>,
    pub selected: usize,
    /// `true` ise seçili sunucunun detay ekranı açık.
    pub detail: bool,
    pub show_help: bool,
    pub should_quit: bool,
}

impl App {
    pub fn new(servers: Vec<ServerSnapshot>) -> Self {
        let mut app = Self {
            states: Vec::new(),
            selected: 0,
            detail: false,
            show_help: false,
            should_quit: false,
        };
        app.set_servers(&servers);
        app
    }

    /// Yeni veriyi alır, geçmişe ekler, seçimi geçerli aralıkta tutar.
    pub fn set_servers(&mut self, servers: &[ServerSnapshot]) {
        self.states.resize_with(servers.len(), Default::default);
        for (st, snap) in self.states.iter_mut().zip(servers) {
            st.push(snap);
        }
        self.selected = self.selected.min(self.states.len().saturating_sub(1));
    }

    pub fn select_next(&mut self) {
        if !self.states.is_empty() {
            self.selected = (self.selected + 1).min(self.states.len() - 1);
        }
    }

    pub fn select_prev(&mut self) {
        self.selected = self.selected.saturating_sub(1);
    }

    pub fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        // Yardım açıkken `?`, Esc ve q yalnızca popup'ı kapatır.
        if self.show_help {
            if matches!(code, KeyCode::Char('?' | 'q') | KeyCode::Esc) {
                self.show_help = false;
            }
            return;
        }
        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('c') if mods.contains(KeyModifiers::CONTROL) => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('j') | KeyCode::Down => self.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.select_prev(),
            KeyCode::Enter if !self.states.is_empty() => self.detail = true,
            KeyCode::Esc => self.detail = false,
            _ => {}
        }
    }
}

/// Terminali açar, olay döngüsünü çalıştırır, çıkışta terminali eski haline getirir.
/// Panic durumunda da `ratatui::init` içindeki hook terminali geri yükler.
pub async fn run(mut data: watch::Receiver<Vec<ServerSnapshot>>) -> Result<()> {
    // color_eyre::install() çağrısı main'de, bundan önce yapılmış olmalı;
    // ratatui::init mevcut panic hook'unu zincirler.
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut data).await;
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    data: &mut watch::Receiver<Vec<ServerSnapshot>>,
) -> Result<()> {
    let mut app = App::new(data.borrow_and_update().clone());
    let mut events = EventStream::new();
    let mut table_state = ratatui::widgets::TableState::default();
    // Yeniden deneme geri sayımı için saniyede bir yeniden çizim.
    let mut redraw = tokio::time::interval(std::time::Duration::from_secs(1));

    while !app.should_quit {
        terminal.draw(|f| {
            if app.detail {
                detail::draw(f, &app);
            } else {
                overview::draw(f, &app, &mut table_state);
            }
        })?;
        tokio::select! {
            // UI veri kaynağını asla bloklamaz: yalnızca son değer okunur.
            changed = data.changed() => {
                if changed.is_ok() {
                    app.set_servers(&data.borrow_and_update());
                }
            }
            _ = redraw.tick() => {}
            ev = events.next() => match ev {
                Some(Ok(Event::Key(k))) if k.kind == KeyEventKind::Press => {
                    app.on_key(k.code, k.modifiers);
                }
                // Resize: döngü başında yeniden çizilir.
                Some(Ok(_)) => {}
                Some(Err(e)) => return Err(e.into()),
                None => break,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    #[test]
    fn enter_esc_detay_acar_kapatir() {
        let mut app = App::new(demo::snapshots(0));
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.detail);
        app.on_key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.detail);
        let mut empty = App::new(vec![]);
        empty.on_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(!empty.detail);
    }
}
