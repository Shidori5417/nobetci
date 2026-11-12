//! ratatui ekranları.

mod detail;
mod logs;
mod overview;

use color_eyre::Result;
use crossterm::event::{Event, EventStream, KeyCode, KeyEventKind, KeyModifiers};
use futures_util::StreamExt;
use tokio::sync::{mpsc, watch};

use crate::actions::{Actions, Outcome};
use crate::logs::LogMsg;
use crate::model::ServerSnapshot;
use crate::state::ServerState;
use logs::{LogKey, LogView};

/// Restart onay popup'ı için bekleyen istek.
pub struct Confirm {
    pub server: usize,
    pub server_name: String,
    pub service: String,
}

/// Durum satırındaki mesajın türü.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Level {
    Info,
    Ok,
    Err,
}

/// UI durumu.
pub struct App {
    /// Sunucu başına son snapshot + geçmiş (UI sahibidir).
    pub states: Vec<ServerState>,
    pub selected: usize,
    /// `true` ise seçili sunucunun detay ekranı açık.
    pub detail: bool,
    pub show_help: bool,
    pub should_quit: bool,
    /// Detayda servis listesi odakta mı (`s`)?
    pub focus_services: bool,
    pub svc_sel: usize,
    pub confirm: Option<Confirm>,
    /// Son işlem sonucu / ipucu; detay ekranının alt satırında gösterilir.
    pub status: Option<(Level, String)>,
    logs: Option<LogView>,
    actions: Actions,
}

impl App {
    pub fn new(servers: Vec<ServerSnapshot>, actions: Actions) -> Self {
        let mut app = Self {
            states: Vec::new(),
            selected: 0,
            detail: false,
            show_help: false,
            should_quit: false,
            focus_services: false,
            svc_sel: 0,
            confirm: None,
            status: None,
            logs: None,
            actions,
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
        self.focus_services = false;
        if !self.states.is_empty() {
            self.selected = (self.selected + 1).min(self.states.len() - 1);
        }
    }

    pub fn select_prev(&mut self) {
        self.focus_services = false;
        self.selected = self.selected.saturating_sub(1);
    }

    /// Seçili sunucunun, listede seçili servisinin adı.
    fn selected_service(&self) -> Option<String> {
        let st = self.states.get(self.selected)?;
        Some(st.latest.services.get(self.svc_sel)?.name.clone())
    }

    /// `r`: onay popup'ı açar (yalnızca servis listesi odaktayken ve işlemler açıksa).
    fn ask_restart(&mut self) {
        if !self.actions.allowed(self.selected) {
            self.status = Some((
                Level::Err,
                "Bu sunucuda işlemler kapalı (allow_actions = false)".into(),
            ));
            return;
        }
        let Some(service) = self.selected_service().filter(|_| self.focus_services) else {
            self.status = Some((
                Level::Info,
                "Önce `s` ile servis listesine geçip servis seçin".into(),
            ));
            return;
        };
        self.confirm = Some(Confirm {
            server: self.selected,
            server_name: self.states[self.selected].latest.name.clone(),
            service,
        });
    }

    /// `l`: odakta servis varsa onun, yoksa tüm günlüğün canlı akışını açar.
    fn open_logs(&mut self) {
        let unit = self.selected_service().filter(|_| self.focus_services);
        let Some(rx) = self.actions.logs(self.selected, unit.clone()) else {
            return;
        };
        let server = &self.states[self.selected].latest.name;
        let title = format!("{server} / {}", unit.as_deref().unwrap_or("tüm günlük"));
        self.logs = Some(LogView::new(title, rx));
    }

    /// Restart işleminin sonucunu durum satırına yazar.
    pub fn on_outcome(&mut self, o: Outcome) {
        self.status = Some(match o.result {
            Ok(m) => (Level::Ok, format!("✓ {}: {m}", o.server)),
            Err(e) => (Level::Err, format!("✗ {}: {e}", o.server)),
        });
    }

    /// Akış iletisi (log görüntüleyici açıksa).
    pub fn on_log(&mut self, msg: Option<LogMsg>) {
        if let Some(l) = &mut self.logs {
            l.on_msg(msg);
        }
    }

    /// Detay ekranına özgü tuşlar; işlendiyse `true`.
    fn detail_key(&mut self, code: KeyCode) -> bool {
        let n = self
            .states
            .get(self.selected)
            .map_or(0, |s| s.latest.services.len());
        match code {
            KeyCode::Char('s') if n > 0 => {
                self.focus_services = true;
                self.svc_sel = self.svc_sel.min(n - 1);
            }
            KeyCode::Char('r') => self.ask_restart(),
            KeyCode::Char('l') => self.open_logs(),
            KeyCode::Char('j') | KeyCode::Down if self.focus_services => {
                self.svc_sel = (self.svc_sel + 1).min(n.saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up if self.focus_services => {
                self.svc_sel = self.svc_sel.saturating_sub(1);
            }
            KeyCode::Esc if self.focus_services => self.focus_services = false,
            KeyCode::Esc => {
                self.detail = false;
                self.status = None;
            }
            _ => return false,
        }
        true
    }

    pub fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        if code == KeyCode::Char('c') && mods.contains(KeyModifiers::CONTROL) {
            self.should_quit = true;
            return;
        }
        // Yardım açıkken `?`, Esc ve q yalnızca popup'ı kapatır.
        if self.show_help {
            if matches!(code, KeyCode::Char('?' | 'q') | KeyCode::Esc) {
                self.show_help = false;
            }
            return;
        }
        // Onay popup'ı: yalnızca `e`/`E` onaylar, başka her tuş (Esc, h, Enter...) vazgeçer.
        if let Some(c) = self.confirm.take() {
            if matches!(code, KeyCode::Char('e' | 'E')) {
                tracing::info!("{}: {} restart onaylandı", c.server_name, c.service);
                self.status = Some((
                    Level::Info,
                    format!("⟳ {}: {} yeniden başlatılıyor...", c.server_name, c.service),
                ));
                self.actions.restart(c.server, c.service);
            }
            return;
        }
        if let Some(l) = &mut self.logs {
            match l.on_key(code) {
                LogKey::Stay => {}
                // Görüntüleyiciyi düşürmek akışı ve uzak journalctl'i durdurur.
                LogKey::Close => self.logs = None,
                LogKey::Help => self.show_help = true,
            }
            return;
        }
        if self.detail && self.detail_key(code) {
            return;
        }
        match code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Char('j') | KeyCode::Down => self.select_next(),
            KeyCode::Char('k') | KeyCode::Up => self.select_prev(),
            KeyCode::Enter if !self.states.is_empty() => {
                self.detail = true;
                self.focus_services = false;
                self.svc_sel = 0;
            }
            KeyCode::Esc => self.detail = false,
            _ => {}
        }
    }
}

/// Terminali açar, olay döngüsünü çalıştırır, çıkışta terminali eski haline getirir.
/// Panic durumunda da `ratatui::init` içindeki hook terminali geri yükler.
pub async fn run(
    mut data: watch::Receiver<Vec<ServerSnapshot>>,
    actions: Actions,
    mut outcomes: mpsc::UnboundedReceiver<Outcome>,
) -> Result<()> {
    // color_eyre::install() çağrısı main'de, bundan önce yapılmış olmalı;
    // ratatui::init mevcut panic hook'unu zincirler.
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut data, actions, &mut outcomes).await;
    ratatui::restore();
    result
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    data: &mut watch::Receiver<Vec<ServerSnapshot>>,
    actions: Actions,
    outcomes: &mut mpsc::UnboundedReceiver<Outcome>,
) -> Result<()> {
    let mut app = App::new(data.borrow_and_update().clone(), actions);
    let mut events = EventStream::new();
    let mut table_state = ratatui::widgets::TableState::default();
    // Yeniden deneme geri sayımı için saniyede bir yeniden çizim.
    let mut redraw = tokio::time::interval(std::time::Duration::from_secs(1));

    while !app.should_quit {
        terminal.draw(|f| {
            if let Some(l) = &app.logs {
                l.draw(f);
                if app.show_help {
                    overview::draw_help(f);
                }
            } else if app.detail {
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
            Some(o) = outcomes.recv() => app.on_outcome(o),
            msg = recv_log(&mut app.logs) => app.on_log(msg),
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

/// Log görüntüleyici açıksa akıştan sonraki ileti; değilse hiç hazır olmaz.
async fn recv_log(logs: &mut Option<LogView>) -> Option<LogMsg> {
    match logs {
        Some(l) => l.recv().await,
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    fn test_actions() -> Actions {
        Actions::new(demo::server_configs(), true).0
    }

    #[test]
    fn enter_esc_detay_acar_kapatir() {
        let mut app = App::new(demo::snapshots(0), test_actions());
        app.on_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(app.detail);
        app.on_key(KeyCode::Esc, KeyModifiers::NONE);
        assert!(!app.detail);
        let mut empty = App::new(vec![], test_actions());
        empty.on_key(KeyCode::Enter, KeyModifiers::NONE);
        assert!(!empty.detail);
    }

    fn key(app: &mut App, c: KeyCode) {
        app.on_key(c, KeyModifiers::NONE);
    }

    fn open_detail(sel: usize) -> App {
        let mut app = App::new(demo::snapshots(0), test_actions());
        app.selected = sel;
        key(&mut app, KeyCode::Enter);
        app
    }

    #[test]
    fn servis_odagi_gezinme_ve_esc() {
        let mut app = open_detail(0);
        key(&mut app, KeyCode::Char('s'));
        assert!(app.focus_services);
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('j'));
        key(&mut app, KeyCode::Char('j'));
        assert_eq!(app.svc_sel, 2); // vds-1: 3 servis
        key(&mut app, KeyCode::Char('k'));
        assert_eq!(app.svc_sel, 1);
        // Odaktayken j/k sunucu değiştirmez; Esc önce odağı bırakır, detaydan çıkmaz.
        assert_eq!(app.selected, 0);
        key(&mut app, KeyCode::Esc);
        assert!(!app.focus_services && app.detail);
        key(&mut app, KeyCode::Esc);
        assert!(!app.detail);
    }

    #[test]
    fn restart_kapali_sunucuda_mesaj_acikta_onay_ve_varsayilan_hayir() {
        let mut off = open_detail(1);
        key(&mut off, KeyCode::Char('s'));
        key(&mut off, KeyCode::Char('r'));
        assert!(off.confirm.is_none());
        let (lvl, msg) = off.status.clone().unwrap();
        assert_eq!(lvl, Level::Err);
        assert!(msg.contains("Bu sunucuda işlemler kapalı (allow_actions = false)"));

        let mut on = open_detail(0);
        // Servis seçmeden `r` popup açmaz.
        key(&mut on, KeyCode::Char('r'));
        assert!(on.confirm.is_none() && on.status.is_some());
        on.status = None;
        key(&mut on, KeyCode::Char('s'));
        key(&mut on, KeyCode::Char('r'));
        assert_eq!(
            on.confirm.as_ref().map(|c| c.service.as_str()),
            Some("nginx")
        );
        // Enter ya da `h` vazgeçer, işlem başlamaz.
        key(&mut on, KeyCode::Enter);
        assert!(on.confirm.is_none() && on.status.is_none());
        key(&mut on, KeyCode::Char('r'));
        key(&mut on, KeyCode::Char('h'));
        assert!(on.confirm.is_none() && on.status.is_none());
    }

    #[tokio::test]
    async fn restart_onayi_islemi_baslatir_ve_sonuc_durum_satirina_duser() {
        let (actions, mut rx) = Actions::new(demo::server_configs(), true);
        let mut app = App::new(demo::snapshots(0), actions);
        key(&mut app, KeyCode::Enter);
        key(&mut app, KeyCode::Char('s'));
        key(&mut app, KeyCode::Char('r'));
        key(&mut app, KeyCode::Char('e'));
        assert_eq!(app.status.as_ref().map(|s| s.0), Some(Level::Info));
        app.on_outcome(rx.recv().await.unwrap());
        let (lvl, msg) = app.status.clone().unwrap();
        assert_eq!(lvl, Level::Ok);
        assert!(msg.contains("vds-1") && msg.contains("nginx"));
    }

    #[tokio::test]
    async fn log_goruntuleyici_acilir_satir_alir_esc_kapatir() {
        let mut app = open_detail(0);
        key(&mut app, KeyCode::Char('l'));
        let msg = app.logs.as_mut().unwrap().recv().await;
        app.on_log(msg);
        assert!(app.logs.is_some());
        // Görüntüleyici açıkken `q` uygulamayı kapatmaz, yalnızca görüntüleyiciyi.
        key(&mut app, KeyCode::Esc);
        assert!(app.logs.is_none() && app.detail && !app.should_quit);
    }
}
