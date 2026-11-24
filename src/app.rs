//! Interface (egui) no estilo do YouTube Music: barra superior com busca,
//! menu lateral, listas/grades e barra do player com progresso fino.

use crate::auth::Auth;
use crate::player::{Player, Status, find_ytdlp};
use crate::updater;
use crate::util::{self, fmt_time};
use crate::ytm::{Client, Playlist, Track, thumb_url};
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

const BG: Color32 = Color32::from_rgb(0x03, 0x03, 0x03);
const PANEL: Color32 = Color32::from_rgb(0x21, 0x21, 0x21);
const SURFACE: Color32 = Color32::from_rgb(0x21, 0x21, 0x21);
const ACCENT: Color32 = Color32::from_rgb(0xff, 0x4e, 0x45);
const YT_RED: Color32 = Color32::from_rgb(0xff, 0x00, 0x00);
const DIM: Color32 = Color32::from_rgb(0xaa, 0xaa, 0xaa);
const AVATAR: Color32 = Color32::from_rgb(0x5b, 0x4a, 0xe0);
const ROW_H: f32 = 60.0;
const THUMB: f32 = 48.0;
const PLAYER_H: f32 = 60.0;
/// Largura minima da janela para mostrar o menu lateral e o layout "desktop".
const WIDE_AT: f32 = 720.0;

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Search,
    Liked,
    Playlists,
}

enum Msg {
    Search(Result<Vec<Track>, String>),
    Liked(Result<Vec<Track>, String>),
    Playlists(Result<Vec<Playlist>, String>),
    PlaylistTracks(Playlist, Result<Vec<Track>, String>),
    Mix(String, Result<Vec<Track>, String>),
    Account(Option<String>),
    Login(Result<Auth, String>),
    /// Resultado da consulta de atualizacao (`manual`: veio do botao, nao da verificacao diaria).
    UpdateChecked { manual: bool, result: Result<Option<updater::Release>, String> },
    UpdateProgress(u64, u64),
    UpdateDownloaded(Result<std::path::PathBuf, String>),
}

#[derive(Clone, PartialEq)]
enum UpdateState {
    Idle,
    Checking,
    UpToDate,
    Downloading { done: u64, total: u64 },
    Installing,
    Failed(String),
}

pub struct App {
    client: Client,
    player: Player,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,

    tab: Tab,
    query: String,
    last_query: String,
    results: Vec<Track>,
    liked: Option<Vec<Track>>,
    playlists: Option<Vec<Playlist>>,
    open_playlist: Option<(Playlist, Vec<Track>)>,
    loading: bool,

    queue: Vec<Track>,
    qi: usize,
    mix_pending: bool,
    consecutive_errors: u8,
    last_title: String,
    seek_drag: Option<f32>,

    repeat: Repeat,
    shuffle: bool,
    volume_before_mute: f32,

    notice: Option<String>,
    show_settings: bool,
    cookie_input: String,
    account_name: Option<String>,
    ytdlp_ok: bool,
    update_state: UpdateState,
    update_settings: updater::Settings,
    /// Versao nova achada pela verificacao diaria, aguardando o usuario aceitar.
    update_available: Option<updater::Release>,
    auto_checking: bool,
    next_auto_check: Instant,
    #[cfg(windows)]
    taskbar: Option<crate::taskbar::Taskbar>,
}

/// Registra os botoes de controle na miniatura da barra de tarefas (so Windows).
#[cfg(windows)]
fn init_taskbar(cc: &eframe::CreationContext<'_>) -> Option<crate::taskbar::Taskbar> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => crate::taskbar::Taskbar::new(h.hwnd.get(), cc.egui_ctx.clone()),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Repeat {
    Off,
    All,
    One,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> App {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        setup_fonts(&cc.egui_ctx);
        setup_style(&cc.egui_ctx);
        let (tx, rx) = channel();
        let client = Client::new(Auth::load());
        let mut app = App {
            player: Player::new(cc.egui_ctx.clone()),
            client,
            tx,
            rx,
            tab: Tab::Search,
            query: String::new(),
            last_query: String::new(),
            results: Vec::new(),
            liked: None,
            playlists: None,
            open_playlist: None,
            loading: false,
            queue: Vec::new(),
            qi: 0,
            mix_pending: false,
            consecutive_errors: 0,
            last_title: String::new(),
            seek_drag: None,
            repeat: Repeat::Off,
            shuffle: false,
            volume_before_mute: 0.5,
            notice: None,
            show_settings: false,
            cookie_input: String::new(),
            account_name: None,
            ytdlp_ok: find_ytdlp().is_some(),
            update_state: UpdateState::Idle,
            update_settings: updater::Settings::load(),
            update_available: None,
            auto_checking: false,
            next_auto_check: Instant::now(),
            #[cfg(windows)]
            taskbar: init_taskbar(cc),
        };
        app.refresh_account();
        app
    }

    /// Play/pause: alterna se ja ha faixa tocando/pausada; senao comeca a fila.
    fn toggle_playback(&mut self, st: &crate::player::PlayerState) {
        if st.current.is_some() && matches!(st.status, Status::Playing | Status::Paused) {
            self.player.toggle();
        } else if !self.queue.is_empty() {
            self.start_current();
        }
    }

    // ------------------------------------------------------------ background

    fn spawn(&self, ctx: &egui::Context, job: impl FnOnce(Client) -> Msg + Send + 'static) {
        let (client, tx, ctx) = (self.client.clone(), self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(job(client));
            ctx.request_repaint();
        });
    }

    fn refresh_account(&mut self) {
        self.account_name = None;
        if self.client.is_logged_in() {
            let (client, tx) = (self.client.clone(), self.tx.clone());
            std::thread::spawn(move || {
                let _ = tx.send(Msg::Account(client.account_name()));
            });
        }
    }

    fn search(&mut self, ctx: &egui::Context) {
        let q = self.query.trim().to_string();
        if q.is_empty() {
            return;
        }
        self.tab = Tab::Search;
        self.open_playlist = None;
        self.loading = true;
        self.notice = None;
        self.last_query = q.clone();
        self.spawn(ctx, move |c| Msg::Search(c.search(&q)));
    }

    fn connect_account(&mut self, ctx: &egui::Context) {
        self.notice = Some("Abrindo a janela de login...".into());
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Login(run_login_window()));
            ctx.request_repaint();
        });
    }

    fn apply_auth(&mut self, auth: Option<Auth>) {
        self.client = Client::new(auth);
        self.liked = None;
        self.playlists = None;
        self.open_playlist = None;
        self.refresh_account();
    }

    fn drain_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Search(r) => {
                    self.loading = false;
                    match r {
                        Ok(t) if t.is_empty() => self.notice = Some("Nada encontrado.".into()),
                        Ok(t) => self.results = t,
                        Err(e) => self.notice = Some(e),
                    }
                }
                Msg::Liked(r) => {
                    self.loading = false;
                    match r {
                        Ok(t) => self.liked = Some(t),
                        Err(e) => {
                            self.liked = Some(Vec::new());
                            self.notice = Some(e);
                        }
                    }
                }
                Msg::Playlists(r) => {
                    self.loading = false;
                    match r {
                        Ok(p) => self.playlists = Some(p),
                        Err(e) => {
                            self.playlists = Some(Vec::new());
                            self.notice = Some(e);
                        }
                    }
                }
                Msg::PlaylistTracks(p, r) => {
                    self.loading = false;
                    match r {
                        Ok(t) => self.open_playlist = Some((p, t)),
                        Err(e) => self.notice = Some(e),
                    }
                }
                Msg::Mix(for_id, r) => {
                    self.mix_pending = false;
                    let still_current = self.queue.get(self.qi).is_some_and(|t| t.video_id == for_id);
                    if let (true, Ok(tracks)) = (still_current, r) {
                        for t in tracks {
                            if !self.queue.iter().any(|q| q.video_id == t.video_id) {
                                self.queue.push(t);
                            }
                        }
                        self.prefetch_next();
                    }
                }
                Msg::Account(name) => self.account_name = name,
                Msg::Login(r) => match r {
                    Ok(auth) => {
                        if let Err(e) = auth.save() {
                            self.notice = Some(format!("Conectado, mas nao salvou: {e}"));
                        } else {
                            self.notice = Some("Conta conectada.".into());
                        }
                        self.apply_auth(Some(auth));
                    }
                    Err(e) => self.notice = Some(e),
                },
                Msg::UpdateChecked { manual, result } => {
                    if !manual {
                        self.auto_checking = false;
                    }
                    match result {
                        Ok(found) => {
                            self.update_settings.last_check = util::unix_secs();
                            self.update_settings.save();
                            // Nunca instala sozinho: so avisa, e o usuario decide no aviso
                            // ou nas configuracoes.
                            match found {
                                Some(rel) => {
                                    if manual {
                                        self.update_state = UpdateState::Idle;
                                    }
                                    self.update_available = Some(rel);
                                }
                                None if manual => self.update_state = UpdateState::UpToDate,
                                None => {}
                            }
                        }
                        Err(e) if manual => self.update_state = UpdateState::Failed(e),
                        // Falha na verificacao diaria: fica quieto e tenta de novo em 1 hora.
                        Err(_) => self.next_auto_check = Instant::now() + Duration::from_secs(3600),
                    }
                }
                Msg::UpdateProgress(done, total) => {
                    if let UpdateState::Downloading { done: d, total: t } = &mut self.update_state {
                        (*d, *t) = (done, total);
                    }
                }
                Msg::UpdateDownloaded(result) => match result {
                    Ok(path) => {
                        self.update_state = UpdateState::Installing;
                        match updater::launch_installer(&path) {
                            // O instalador precisa substituir os arquivos: o app fecha e ele o reabre.
                            Ok(()) => std::process::exit(0),
                            Err(e) => self.update_state = UpdateState::Failed(e),
                        }
                    }
                    Err(e) => self.update_state = UpdateState::Failed(e),
                },
            }
        }
    }

    // ---------------------------------------------------------------- atualizacao

    fn update_busy(&self) -> bool {
        matches!(self.update_state, UpdateState::Checking | UpdateState::Downloading { .. } | UpdateState::Installing)
    }

    /// `manual`: botao "Verificar atualizacao" (mostra o resultado nas configuracoes).
    /// Automatico: roda em segundo plano e fica quieto se nao houver nada novo.
    /// Nos dois casos, se houver versao nova so avisa; instalar e sempre escolha do usuario.
    fn check_for_update(&mut self, ctx: &egui::Context, manual: bool) {
        if self.update_busy() || (!manual && self.auto_checking) {
            return;
        }
        if manual {
            self.update_state = UpdateState::Checking;
        } else {
            self.auto_checking = true;
        }
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Msg::UpdateChecked { manual, result: updater::fetch_latest() });
            ctx.request_repaint();
        });
    }

    fn start_update(&mut self, rel: updater::Release, ctx: &egui::Context) {
        self.update_available = None;
        self.update_state = UpdateState::Downloading { done: 0, total: rel.size };
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let progress_tx = tx.clone();
            let progress_ctx = ctx.clone();
            let result = updater::download(&rel, move |done, total| {
                let _ = progress_tx.send(Msg::UpdateProgress(done, total));
                progress_ctx.request_repaint();
            });
            let _ = tx.send(Msg::UpdateDownloaded(result));
            ctx.request_repaint();
        });
    }

    /// Verificacao diaria: roda ao abrir (se passou um dia) e, com o app aberto por dias, a cada hora reavalia.
    fn maybe_auto_check(&mut self, ctx: &egui::Context) {
        if !self.update_settings.auto_check || self.update_busy() || self.update_available.is_some() || self.auto_checking {
            return;
        }
        if self.update_settings.due(util::unix_secs()) && Instant::now() >= self.next_auto_check {
            self.check_for_update(ctx, false);
        } else {
            ctx.request_repaint_after(Duration::from_secs(3600));
        }
    }

    fn update_banner(&mut self, ui: &mut egui::Ui) {
        let Some(rel) = self.update_available.clone() else { return };
        let mut accept = false;
        let mut dismiss = false;
        egui::Frame::new().fill(Color32::from_rgb(0x14, 0x24, 0x3a)).corner_radius(8.0).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Nova versão {} disponível", rel.version)).strong());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("Depois").clicked() {
                        dismiss = true;
                    }
                    if pill_button(ui, "Baixar e instalar", true).clicked() {
                        accept = true;
                    }
                });
            });
        });
        ui.add_space(6.0);
        if accept {
            self.start_update(rel, &ui.ctx().clone());
        } else if dismiss {
            self.update_available = None;
        }
    }

    fn update_section(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Atualizações").strong().size(15.0));
        ui.label(RichText::new(format!("Versão instalada: {}", updater::CURRENT)).color(DIM).small());
        ui.add_space(4.0);
        match self.update_state.clone() {
            UpdateState::Idle => {}
            UpdateState::Checking => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Verificando...");
                });
            }
            UpdateState::UpToDate => {
                ui.label("Você já está na versão mais recente.");
            }
            UpdateState::Downloading { done, total } => {
                ui.label("Baixando a atualização...");
                let frac = if total > 0 { (done as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
                ui.add(egui::ProgressBar::new(frac).show_percentage());
            }
            UpdateState::Installing => {
                ui.label("Instalando... o app vai fechar e abrir de novo.");
            }
            UpdateState::Failed(e) => {
                ui.colored_label(ACCENT, e);
            }
        }
        if let Some(rel) = self.update_available.clone() {
            ui.label(format!("Nova versão {} disponível.", rel.version));
            if pill_button(ui, "Baixar e instalar", true).clicked() {
                self.start_update(rel, &ui.ctx().clone());
            }
        }
        ui.add_space(4.0);
        let busy = self.update_busy();
        if ui.add_enabled(!busy, egui::Button::new("Verificar atualização")).clicked() {
            self.check_for_update(&ui.ctx().clone(), true);
        }
        let mut auto = self.update_settings.auto_check;
        if ui.checkbox(&mut auto, "Verificar automaticamente uma vez por dia").changed() {
            self.update_settings.auto_check = auto;
            self.update_settings.save();
        }
    }

    // ----------------------------------------------------------------- queue

    fn play_from(&mut self, list: &[Track], index: usize, with_mix: bool, ctx: &egui::Context) {
        let Some(track) = list.get(index).cloned() else { return };
        self.consecutive_errors = 0;
        if with_mix {
            self.queue = vec![track.clone()];
            self.qi = 0;
            self.request_mix(ctx);
        } else {
            self.queue = list.to_vec();
            self.qi = index;
            if self.shuffle {
                self.shuffle_upcoming();
            }
        }
        self.start_current();
    }

    /// Embaralha so o que vem depois da faixa atual.
    fn shuffle_upcoming(&mut self) {
        let from = (self.qi + 1).min(self.queue.len());
        shuffle_in_place(&mut self.queue[from..]);
        self.prefetch_next();
    }

    fn start_current(&mut self) {
        if let Some(t) = self.queue.get(self.qi).cloned() {
            self.player.play(t);
            self.prefetch_next();
        }
    }

    fn prefetch_next(&self) {
        if let Some(next) = self.queue.get(self.qi + 1) {
            self.player.prefetch(&next.video_id);
        }
    }

    fn request_mix(&mut self, ctx: &egui::Context) {
        if self.mix_pending {
            return;
        }
        let Some(id) = self.queue.get(self.qi).map(|t| t.video_id.clone()) else { return };
        self.mix_pending = true;
        self.spawn(ctx, move |c| {
            let r = c.mix(&id);
            Msg::Mix(id, r)
        });
    }

    fn next(&mut self, ctx: &egui::Context) {
        if self.qi + 1 < self.queue.len() {
            self.qi += 1;
            self.start_current();
        } else if self.repeat == Repeat::All && !self.queue.is_empty() {
            self.qi = 0;
            self.start_current();
        } else if !self.queue.is_empty() {
            // Fim da fila: continua com o mix da ultima faixa.
            self.request_mix(ctx);
        }
    }

    fn prev(&mut self) {
        let pos = self.player.snapshot().position;
        if pos > Duration::from_secs(3) || self.qi == 0 {
            self.player.seek(Duration::ZERO);
        } else {
            self.qi -= 1;
            self.start_current();
        }
    }

    // ------------------------------------------------------------------ nav

    fn switch_tab(&mut self, tab: Tab, ctx: &egui::Context) {
        self.tab = tab;
        self.open_playlist = None;
        self.load_tab(ctx);
    }

    fn load_tab(&mut self, ctx: &egui::Context) {
        if !self.client.is_logged_in() {
            return;
        }
        match self.tab {
            Tab::Liked if self.liked.is_none() && !self.loading => {
                self.loading = true;
                self.spawn(ctx, |c| Msg::Liked(c.liked()));
            }
            Tab::Playlists if self.playlists.is_none() && !self.loading => {
                self.loading = true;
                self.spawn(ctx, |c| Msg::Playlists(c.library_playlists()));
            }
            _ => {}
        }
    }

    // ---------------------------------------------------------------- widgets

    fn top_bar(&mut self, ui: &mut egui::Ui, wide: bool) {
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 12.0;
            logo(ui, wide);
            let right_w = 34.0 + 24.0 + ui.spacing().item_spacing.x * 2.0;
            let avail = (ui.available_width() - right_w).max(120.0);
            let search_w = avail.min(560.0);
            ui.add_space(((avail - search_w) / 2.0).max(0.0));
            self.search_box(ui, search_w);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.avatar(ui);
                if self.loading {
                    ui.spinner();
                }
            });
        });
        if !wide {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                for (tab, label) in NAV {
                    if chip(ui, label, self.tab == tab) {
                        self.switch_tab(tab, &ui.ctx().clone());
                    }
                }
            });
        }
        ui.add_space(10.0);
    }

    fn search_box(&mut self, ui: &mut egui::Ui, width: f32) {
        egui::Frame::new().fill(SURFACE).corner_radius(20.0).inner_margin(Margin::symmetric(14, 3)).show(ui, |ui| {
            ui.set_width((width - 28.0).max(60.0));
            ui.horizontal(|ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                nav_icon(ui.painter(), NavIcon::Search, rect.center(), DIM);
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .hint_text("Pesquisar músicas, artistas...")
                        .font(FontId::proportional(15.0))
                        .desired_width(f32::INFINITY)
                        .frame(egui::Frame::NONE)
                        .margin(vec2(0.0, 7.0)),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.search(&ui.ctx().clone());
                }
            });
        });
    }

    fn avatar(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
        let logged = self.client.is_logged_in();
        let c = rect.center();
        let fill = if logged { AVATAR } else { Color32::from_gray(70) };
        ui.painter().circle_filled(c, 17.0, fill);
        if resp.hovered() || self.show_settings {
            ui.painter().circle_stroke(c, 17.0, Stroke::new(2.0, Color32::from_white_alpha(90)));
        }
        let initial = self.account_name.as_deref().and_then(|n| n.chars().next()).map(|ch| ch.to_uppercase().to_string());
        match (logged, initial) {
            (true, Some(i)) => {
                ui.painter().text(c, Align2::CENTER_CENTER, i, FontId::proportional(16.0), Color32::WHITE);
            }
            _ => {
                // Silhueta de pessoa.
                let p = ui.painter().with_clip_rect(rect);
                p.circle_filled(pos2(c.x, c.y - 4.0), 5.0, Color32::from_gray(200));
                p.circle_filled(pos2(c.x, c.y + 12.0), 9.0, Color32::from_gray(200));
            }
        }
        if resp.on_hover_text("Conta e ajustes").clicked() {
            self.show_settings = !self.show_settings;
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(6.0);
        for (tab, label) in NAV {
            let icon = match tab {
                Tab::Search => NavIcon::Search,
                Tab::Liked => NavIcon::Heart,
                Tab::Playlists => NavIcon::List,
            };
            if nav_item(ui, label, icon, self.tab == tab) {
                self.switch_tab(tab, &ui.ctx().clone());
            }
        }
    }

    fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.show_settings {
            return;
        }
        let mut open = true;
        egui::Window::new("Conta")
            .open(&mut open)
            .anchor(Align2::RIGHT_TOP, vec2(-12.0, 56.0))
            .collapsible(false)
            .resizable(false)
            .default_width(320.0)
            .show(ctx, |ui| self.settings(ui));
        if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.show_settings = false;
        }
    }

    fn settings(&mut self, ui: &mut egui::Ui) {
        ui.set_width(300.0);
        if self.client.is_logged_in() {
            let who = self.account_name.clone().unwrap_or_else(|| "conta conectada".into());
            ui.label(RichText::new(who).strong().size(16.0));
            ui.label(RichText::new("Conectado ao YouTube Music").color(DIM).small());
            ui.add_space(6.0);
            if ui.button("Desconectar").clicked() {
                Auth::clear();
                self.apply_auth(None);
                self.notice = Some("Conta desconectada.".into());
            }
        } else {
            ui.label("Conecte sua conta para ver curtidas e playlists.");
            ui.add_space(6.0);
            if pill_button(ui, "Conectar conta do YouTube", true).clicked() {
                self.connect_account(&ui.ctx().clone());
                self.show_settings = false;
            }
        }
        ui.add_space(8.0);
        ui.collapsing("Importar cookie manualmente", |ui| {
            ui.label(RichText::new("Cole o cabeçalho Cookie de music.youtube.com (DevTools > Rede).").color(DIM).small());
            ui.add(egui::TextEdit::multiline(&mut self.cookie_input).desired_rows(3).desired_width(f32::INFINITY));
            if ui.button("Importar").clicked() {
                match Auth::from_cookie_header(&self.cookie_input) {
                    Some(auth) => {
                        let _ = auth.save();
                        self.apply_auth(Some(auth));
                        self.cookie_input.clear();
                        self.notice = Some("Cookie importado.".into());
                        self.show_settings = false;
                    }
                    None => self.notice = Some("Cookie invalido: falta o SAPISID.".into()),
                }
            }
        });
        if !self.ytdlp_ok {
            ui.add_space(6.0);
            ui.colored_label(ACCENT, "yt-dlp não encontrado. Instale: winget install yt-dlp.yt-dlp");
        }
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        self.update_section(ui);
    }

    fn track_list(&mut self, ui: &mut egui::Ui, tracks: &[Track], with_mix: bool) {
        let st = self.player.snapshot();
        let current = st.current.map(|t| t.video_id);
        let playing = st.status == Status::Playing;
        let show_album = ui.available_width() >= 560.0;
        let mut clicked = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, tracks.len(), |ui, range| {
            for i in range {
                let t = &tracks[i];
                let is_current = current.as_deref() == Some(t.video_id.as_str());
                if track_row(ui, t, is_current, playing, show_album) {
                    clicked = Some(i);
                }
            }
        });
        if let Some(i) = clicked {
            let ctx = ui.ctx().clone();
            self.play_from(tracks, i, with_mix, &ctx);
        }
    }

    fn playlist_grid(&mut self, ui: &mut egui::Ui, list: &[Playlist]) {
        let gap = 16.0;
        let avail = ui.available_width();
        let cols = (((avail + gap) / (160.0 + gap)).floor() as usize).max(1);
        let card_w = ((avail - gap * (cols as f32 - 1.0)) / cols as f32).floor() - 1.0;
        let row_h = card_w + 64.0;
        let rows = list.len().div_ceil(cols);
        let mut open = None;
        ui.spacing_mut().item_spacing = vec2(gap, 0.0);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, row_h, rows, |ui, range| {
            for r in range {
                ui.horizontal(|ui| {
                    for c in 0..cols {
                        let Some(p) = list.get(r * cols + c) else { break };
                        let (rect, resp) = ui.allocate_exact_size(vec2(card_w, row_h), Sense::click());
                        let img = Rect::from_min_size(rect.min, Vec2::splat(card_w));
                        paint_cover(ui, img, &p.thumb, 8.0, 360);
                        if resp.hovered() {
                            ui.painter().rect_filled(img, 8.0, Color32::from_black_alpha(90));
                            play_badge(ui.painter(), img.center(), 24.0);
                        }
                        paint_text(ui, pos2(rect.min.x, img.bottom() + 8.0), &p.title, FontId::proportional(15.0), Color32::WHITE, card_w);
                        paint_text(ui, pos2(rect.min.x, img.bottom() + 29.0), &p.subtitle, FontId::proportional(13.0), DIM, card_w);
                        if resp.clicked() {
                            open = Some(p.clone());
                        }
                    }
                });
            }
        });
        if let Some(p) = open {
            self.loading = true;
            let id = p.browse_id.clone();
            self.spawn(&ui.ctx().clone(), move |c| Msg::PlaylistTracks(p, c.playlist(&id)));
        }
    }

    fn content(&mut self, ui: &mut egui::Ui, wide: bool) {
        match self.tab {
            Tab::Search => {
                if self.results.is_empty() && !self.loading {
                    empty_state(ui, "Busque suas músicas favoritas", "Digite o nome de uma música ou artista e aperte Enter.");
                } else {
                    let title = if self.last_query.is_empty() { "Resultados".to_string() } else { format!("Resultados para “{}”", self.last_query) };
                    page_header(ui, &title, &format!("{} músicas", self.results.len()), wide, false);
                    let list = self.results.clone();
                    self.track_list(ui, &list, true);
                }
            }
            Tab::Liked => {
                if !self.client.is_logged_in() {
                    return self.needs_login(ui);
                }
                match self.liked.clone() {
                    Some(list) if list.is_empty() => empty_state(ui, "Nenhuma música curtida", "As músicas que você curtir aparecem aqui."),
                    Some(list) => {
                        if page_header(ui, "Músicas que você curtiu", &format!("{} músicas", list.len()), wide, true) {
                            let ctx = ui.ctx().clone();
                            self.play_from(&list, 0, false, &ctx);
                        }
                        self.track_list(ui, &list, false);
                    }
                    None => empty_state(ui, "Carregando...", ""),
                }
            }
            Tab::Playlists => {
                if !self.client.is_logged_in() {
                    return self.needs_login(ui);
                }
                if let Some((p, tracks)) = self.open_playlist.clone() {
                    self.playlist_header(ui, &p, &tracks, wide);
                    self.track_list(ui, &tracks, false);
                } else {
                    match self.playlists.clone() {
                        Some(list) if list.is_empty() => empty_state(ui, "Nenhuma playlist", "Suas playlists da biblioteca aparecem aqui."),
                        Some(list) => {
                            page_header(ui, "Playlists", &format!("{} na sua biblioteca", list.len()), wide, false);
                            self.playlist_grid(ui, &list);
                        }
                        None => empty_state(ui, "Carregando...", ""),
                    }
                }
            }
        }
    }

    fn playlist_header(&mut self, ui: &mut egui::Ui, p: &Playlist, tracks: &[Track], wide: bool) {
        ui.add_space(10.0);
        if chip(ui, "‹ Voltar", false) {
            self.open_playlist = None;
            return;
        }
        ui.add_space(10.0);
        let cover = if wide { 160.0 } else { 96.0 };
        let mut play = false;
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(cover), Sense::hover());
            paint_cover(ui, rect, &p.thumb, 8.0, 360);
            ui.vertical(|ui| {
                ui.add_space(if wide { 8.0 } else { 0.0 });
                ui.add(egui::Label::new(RichText::new(&p.title).size(if wide { 30.0 } else { 20.0 }).strong()).truncate());
                let sub = if p.subtitle.is_empty() { format!("{} músicas", tracks.len()) } else { format!("{} • {} músicas", p.subtitle, tracks.len()) };
                ui.add(egui::Label::new(RichText::new(sub).color(DIM)).truncate());
                ui.add_space(10.0);
                play = pill_button(ui, "▶  Reproduzir", true).clicked();
            });
        });
        ui.add_space(12.0);
        if play {
            let ctx = ui.ctx().clone();
            self.play_from(tracks, 0, false, &ctx);
        }
    }

    fn needs_login(&mut self, ui: &mut egui::Ui) {
        empty_state(ui, "Conecte sua conta", "Entre com sua conta do YouTube para ver esta aba.");
        ui.vertical_centered(|ui| {
            if pill_button(ui, "Conectar conta do YouTube", true).clicked() {
                self.connect_account(&ui.ctx().clone());
            }
        });
    }

    fn notice_bar(&mut self, ui: &mut egui::Ui) {
        let Some(n) = self.notice.clone() else { return };
        egui::Frame::new().fill(Color32::from_rgb(0x2e, 0x16, 0x16)).corner_radius(8.0).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.add(egui::Label::new(RichText::new(n).color(Color32::from_rgb(0xff, 0x9a, 0x92))).wrap());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.small_button("✕").clicked() {
                        self.notice = None;
                    }
                });
            });
        });
        ui.add_space(6.0);
    }

    // ------------------------------------------------------------ player bar

    fn player_bar(&mut self, ui: &mut egui::Ui, wide: bool) {
        let st = self.player.snapshot();
        let ctx = ui.ctx().clone();
        let has_track = st.current.is_some();

        // Progresso: linha fina vermelha colada na borda de cima.
        let dur = st.duration.as_secs_f32().max(1.0);
        let pos = self.seek_drag.unwrap_or(st.position.as_secs_f32()).min(dur);
        let w = ui.available_width();
        if let Some(o) = thin_bar(ui, w, pos / dur, has_track, YT_RED) {
            let v = o.frac * dur;
            if o.committed {
                self.seek_drag = None;
                self.player.seek(Duration::from_secs_f32(v));
            } else {
                self.seek_drag = Some(v);
            }
        }

        let time = format!("{} / {}", fmt_time(pos as u64), fmt_time(dur as u64));
        // Mesmo esquema do YouTube Music: [controles + tempo] [faixa atual centralizada] [volume/repetir/aleatorio].
        // As laterais tem a mesma largura para o centro ficar realmente no meio da janela.
        let roomy = w >= 860.0;
        egui::Frame::new().inner_margin(Margin::symmetric(if wide { 16 } else { 12 }, 8)).show(ui, |ui| {
            if roomy {
                let spacing = ui.spacing().item_spacing.x;
                let total = ui.available_width();
                let side_w = 300.0;
                let mid_w = (total - side_w * 2.0 - spacing * 2.0).max(120.0);
                ui.horizontal(|ui| {
                    ui.allocate_ui_with_layout(vec2(side_w, PLAYER_H), Layout::left_to_right(Align::Center), |ui| {
                        ui.set_min_size(vec2(side_w, PLAYER_H));
                        self.controls(ui, &st, &ctx);
                        ui.add_space(8.0);
                        ui.label(RichText::new(&time).color(DIM).size(13.0));
                    });
                    ui.allocate_ui_with_layout(vec2(mid_w, PLAYER_H), Layout::left_to_right(Align::Center), |ui| {
                        ui.set_min_size(vec2(mid_w, PLAYER_H));
                        let block = mid_w.min(480.0);
                        ui.add_space((mid_w - block) / 2.0);
                        ui.allocate_ui_with_layout(vec2(block, PLAYER_H), Layout::left_to_right(Align::Center), |ui| {
                            now_playing(ui, &st);
                        });
                    });
                    ui.allocate_ui_with_layout(vec2(side_w, PLAYER_H), Layout::right_to_left(Align::Center), |ui| {
                        ui.set_min_size(vec2(side_w, PLAYER_H));
                        self.extras(ui, &st, &ctx, true);
                    });
                });
            } else {
                ui.allocate_ui_with_layout(vec2(ui.available_width(), 50.0), Layout::left_to_right(Align::Center), |ui| {
                    now_playing(ui, &st);
                });
                ui.horizontal(|ui| {
                    self.controls(ui, &st, &ctx);
                    ui.label(RichText::new(&time).color(DIM).size(12.0));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        self.extras(ui, &st, &ctx, false);
                    });
                });
            }
        });
    }

    /// Anterior, play/pause e proxima.
    fn controls(&mut self, ui: &mut egui::Ui, st: &crate::player::PlayerState, ctx: &egui::Context) {
        ui.spacing_mut().item_spacing.x = 4.0;
        if icon_button(ui, Icon::Prev, 40.0).clicked() {
            self.prev();
        }
        let playing = st.status == Status::Playing;
        let main_icon = if playing { Icon::Pause } else { Icon::Play };
        if icon_button(ui, main_icon, 52.0).clicked() {
            self.toggle_playback(st);
        }
        if icon_button(ui, Icon::Next, 40.0).clicked() {
            self.next(ctx);
        }
    }

    /// Lado direito, da esquerda para a direita: [volume] [repetir] [aleatorio] [Mix].
    /// O layout e right_to_left, entao os itens sao adicionados na ordem inversa.
    fn extras(&mut self, ui: &mut egui::Ui, st: &crate::player::PlayerState, ctx: &egui::Context, with_volume: bool) {
        ui.spacing_mut().item_spacing.x = 4.0;

        let resp = ui.add_enabled(st.current.is_some(), egui::Button::new(RichText::new("Mix").strong()).corner_radius(16.0).min_size(vec2(52.0, 30.0)));
        if resp.on_hover_text("Fila infinita a partir desta música").clicked() {
            if let Some(cur) = st.current.clone() {
                self.queue = vec![cur];
                self.qi = 0;
                self.request_mix(ctx);
            }
        }

        let shuffle_color = if self.shuffle { Color32::WHITE } else { DIM };
        let shuffle_tip = if self.shuffle { "Aleatório: ligado" } else { "Aleatório: desligado" };
        if icon_button_color(ui, Icon::Shuffle, 38.0, shuffle_color).on_hover_text(shuffle_tip).clicked() {
            self.shuffle = !self.shuffle;
            if self.shuffle {
                self.shuffle_upcoming();
            }
        }

        let (repeat_icon, repeat_color, repeat_tip) = match self.repeat {
            Repeat::Off => (Icon::Repeat, DIM, "Repetir: desligado"),
            Repeat::All => (Icon::Repeat, Color32::WHITE, "Repetir: fila"),
            Repeat::One => (Icon::RepeatOne, Color32::WHITE, "Repetir: faixa"),
        };
        if icon_button_color(ui, repeat_icon, 38.0, repeat_color).on_hover_text(repeat_tip).clicked() {
            self.repeat = match self.repeat {
                Repeat::Off => Repeat::All,
                Repeat::All => Repeat::One,
                Repeat::One => Repeat::Off,
            };
        }

        if with_volume {
            ui.add_space(8.0);
            if let Some(o) = thin_bar(ui, 96.0, st.volume, true, Color32::WHITE) {
                self.player.set_volume(o.frac);
            }
            let muted = st.volume < 0.01;
            let vol_icon = if muted { Icon::VolumeMuted } else { Icon::Volume };
            if icon_button_color(ui, vol_icon, 38.0, Color32::from_gray(225)).on_hover_text("Silenciar").clicked() {
                if muted {
                    self.player.set_volume(self.volume_before_mute.max(0.3));
                } else {
                    self.volume_before_mute = st.volume;
                    self.player.set_volume(0.0);
                }
            }
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let wide = ui.available_width() >= WIDE_AT;
        self.drain_messages();
        self.maybe_auto_check(&ctx);

        // Avanca a fila quando a faixa termina (ou falha).
        let st = self.player.snapshot();
        if st.finished {
            self.player.state.lock().unwrap().finished = false;
            if let Status::Error(msg) = &st.status {
                self.consecutive_errors += 1;
                self.notice = Some(msg.clone());
                if self.consecutive_errors < 3 {
                    self.next(&ctx);
                }
            } else {
                self.consecutive_errors = 0;
                if self.repeat == Repeat::One {
                    self.start_current();
                } else {
                    self.next(&ctx);
                }
            }
        }
        // Botoes da miniatura na barra de tarefas (anterior / play-pause / proxima).
        #[cfg(windows)]
        {
            let playing = st.status == Status::Playing;
            let actions = self.taskbar.as_mut().map(|tb| tb.poll(playing)).unwrap_or_default();
            for action in actions {
                match action {
                    crate::taskbar::Action::Prev => self.prev(),
                    crate::taskbar::Action::Toggle => self.toggle_playback(&st),
                    crate::taskbar::Action::Next => self.next(&ctx),
                }
            }
        }
        if matches!(st.status, Status::Playing | Status::Loading) {
            ctx.request_repaint_after(Duration::from_millis(200));
        }

        // Titulo da janela acompanha a faixa.
        let title = match &st.current {
            Some(t) if st.status == Status::Playing => format!("{} - {}", t.artists, t.title),
            _ => "YT Music".to_string(),
        };
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }

        let side = Margin::symmetric(if wide { 16 } else { 12 }, 0);
        egui::Panel::top("top").frame(egui::Frame::new().fill(BG).inner_margin(side)).show(ui, |ui| {
            self.top_bar(ui, wide);
        });
        egui::Panel::bottom("player").frame(egui::Frame::new().fill(PANEL)).show(ui, |ui| {
            self.player_bar(ui, wide);
        });
        if wide {
            egui::Panel::left("nav")
                .resizable(false)
                .exact_size(210.0)
                .frame(egui::Frame::new().fill(BG).inner_margin(Margin::symmetric(8, 0)))
                .show(ui, |ui| self.sidebar(ui));
        }
        let content_margin = Margin::symmetric(if wide { 24 } else { 12 }, 0);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(content_margin)).show(ui, |ui| {
            self.update_banner(ui);
            self.notice_bar(ui);
            self.content(ui, wide);
        });
        self.settings_window(&ctx);
    }
}

// ------------------------------------------------------------------- helpers

const NAV: [(Tab, &str); 3] = [(Tab::Search, "Buscar"), (Tab::Liked, "Curtidas"), (Tab::Playlists, "Playlists")];

/// Titulo grande da pagina, com botao "Reproduzir" opcional. Devolve true se clicado.
fn page_header(ui: &mut egui::Ui, title: &str, sub: &str, wide: bool, show_play: bool) -> bool {
    let mut play = false;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.add(egui::Label::new(RichText::new(title).size(if wide { 28.0 } else { 22.0 }).strong()).truncate());
            if !sub.is_empty() {
                ui.label(RichText::new(sub).color(DIM));
            }
        });
        if show_play {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                play = pill_button(ui, "▶  Reproduzir", true).clicked();
            });
        }
    });
    ui.add_space(12.0);
    play
}

fn empty_state(ui: &mut egui::Ui, title: &str, sub: &str) {
    ui.add_space(60.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(title).size(20.0).strong());
        if !sub.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new(sub).color(DIM));
        }
        ui.add_space(12.0);
    });
}

/// Botao arredondado. `primary`: branco com texto preto (como "Reproduzir").
fn pill_button(ui: &mut egui::Ui, text: &str, primary: bool) -> egui::Response {
    let (fill, color) = if primary { (Color32::WHITE, Color32::BLACK) } else { (Color32::from_white_alpha(24), Color32::WHITE) };
    ui.add(egui::Button::new(RichText::new(text).color(color).strong()).fill(fill).corner_radius(18.0).min_size(vec2(0.0, 36.0)))
}

/// Chip de filtro/navegacao: selecionado fica branco com texto preto.
fn chip(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let (fill, color) = if selected { (Color32::WHITE, Color32::BLACK) } else { (Color32::from_white_alpha(24), Color32::WHITE) };
    ui.add(egui::Button::new(RichText::new(label).color(color)).fill(fill).corner_radius(8.0).min_size(vec2(0.0, 32.0))).clicked()
}

#[derive(Clone, Copy)]
enum NavIcon {
    Search,
    Heart,
    List,
}

fn nav_icon(p: &egui::Painter, icon: NavIcon, c: Pos2, color: Color32) {
    let stroke = Stroke::new(2.0, color);
    match icon {
        NavIcon::Search => {
            p.circle_stroke(c + vec2(-1.5, -1.5), 6.0, stroke);
            p.line_segment([c + vec2(2.8, 2.8), c + vec2(7.5, 7.5)], stroke);
        }
        NavIcon::Heart => {
            p.text(c, Align2::CENTER_CENTER, "♥", FontId::proportional(18.0), color);
        }
        NavIcon::List => {
            for (i, w) in [8.0, 8.0, 4.0].into_iter().enumerate() {
                let y = c.y - 5.0 + i as f32 * 5.0;
                p.line_segment([pos2(c.x - 8.0, y), pos2(c.x - 8.0 + w * 2.0, y)], stroke);
            }
        }
    }
}

fn nav_item(ui: &mut egui::Ui, label: &str, icon: NavIcon, selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 42.0), Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 10.0, Color32::from_white_alpha(26));
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 10.0, Color32::from_white_alpha(12));
    }
    let color = if selected || resp.hovered() { Color32::WHITE } else { Color32::from_gray(210) };
    nav_icon(ui.painter(), icon, pos2(rect.min.x + 26.0, rect.center().y), color);
    ui.painter().text(pos2(rect.min.x + 50.0, rect.center().y), Align2::LEFT_CENTER, label, FontId::proportional(15.0), color);
    resp.clicked()
}

/// Logo: circulo vermelho com play + texto "Music".
fn logo(ui: &mut egui::Ui, with_text: bool) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(30.0), Sense::hover());
    let c = rect.center();
    let p = ui.painter();
    p.circle_filled(c, 14.0, YT_RED);
    p.circle_stroke(c, 8.5, Stroke::new(1.6, Color32::WHITE));
    p.add(egui::Shape::convex_polygon(
        vec![c + vec2(-2.6, -4.2), c + vec2(-2.6, 4.2), c + vec2(4.4, 0.0)],
        Color32::WHITE,
        Stroke::NONE,
    ));
    if with_text {
        ui.label(RichText::new("Music").size(21.0).strong());
    }
}

fn paint_cover(ui: &egui::Ui, rect: Rect, thumb: &str, radius: f32, size: u32) {
    if thumb.is_empty() {
        ui.painter().rect_filled(rect, radius, Color32::from_gray(40));
    } else {
        egui::Image::new(thumb_url(thumb, size)).corner_radius(radius).paint_at(ui, rect);
    }
}

/// Circulo escuro com triangulo de play, para sobrepor capas.
fn play_badge(p: &egui::Painter, c: Pos2, r: f32) {
    p.circle_filled(c, r, Color32::from_black_alpha(170));
    p.circle_stroke(c, r, Stroke::new(1.0, Color32::from_white_alpha(60)));
    let s = r * 0.42;
    p.add(egui::Shape::convex_polygon(
        vec![c + vec2(-s * 0.6, -s), c + vec2(-s * 0.6, s), c + vec2(s, 0.0)],
        Color32::WHITE,
        Stroke::NONE,
    ));
}

/// Fisher-Yates com xorshift semeado pelo relogio (nao vale a pena uma crate so para isso).
fn shuffle_in_place<T>(items: &mut [T]) {
    let mut x = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9e37_79b9_7f4a_7c15)
        | 1;
    for i in (1..items.len()).rev() {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        items.swap(i, (x % (i as u64 + 1)) as usize);
    }
}

fn paint_row_bg(ui: &egui::Ui, rect: Rect, resp: &egui::Response, active: bool) {
    if resp.hovered() {
        ui.painter().rect_filled(rect, 8.0, Color32::from_white_alpha(16));
    } else if active {
        ui.painter().rect_filled(rect, 8.0, Color32::from_white_alpha(9));
    }
}

/// Texto numa unica linha, truncado com reticencias.
fn paint_text(ui: &egui::Ui, pos: Pos2, text: &str, font: FontId, color: Color32, max_w: f32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap.max_width = max_w.max(10.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(pos, galley, color);
}

fn track_row(ui: &mut egui::Ui, t: &Track, is_current: bool, playing: bool, show_album: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
    paint_row_bg(ui, rect, &resp, is_current);

    // Capa; com hover mostra play, na faixa atual mostra o equalizador.
    let img = Rect::from_min_size(rect.min + vec2(8.0, (ROW_H - THUMB) / 2.0), Vec2::splat(THUMB));
    paint_cover(ui, img, &t.thumb, 6.0, 120);
    if resp.hovered() {
        ui.painter().rect_filled(img, 6.0, Color32::from_black_alpha(120));
        play_badge(ui.painter(), img.center(), 13.0);
    } else if is_current {
        ui.painter().rect_filled(img, 6.0, Color32::from_black_alpha(130));
        equalizer(ui, img.center(), playing);
    }

    let dur = if t.duration_secs > 0 { fmt_time(t.duration_secs as u64) } else { String::new() };
    ui.painter().text(pos2(rect.right() - 12.0, rect.center().y), Align2::RIGHT_CENTER, &dur, FontId::proportional(13.0), DIM);

    let tx = img.right() + 14.0;
    let text_right = rect.right() - 12.0 - 48.0;
    let total = (text_right - tx).max(40.0);
    let title_color = Color32::WHITE;
    if show_album {
        let title_w = total * 0.58 - 8.0;
        paint_text(ui, pos2(tx, rect.min.y + 11.0), &t.title, FontId::proportional(15.0), title_color, title_w);
        paint_text(ui, pos2(tx, rect.min.y + 33.0), &t.artists, FontId::proportional(13.0), DIM, title_w);
        paint_text(ui, pos2(tx + total * 0.58 + 8.0, rect.center().y - 9.0), &t.album, FontId::proportional(14.0), DIM, total * 0.42 - 8.0);
    } else {
        paint_text(ui, pos2(tx, rect.min.y + 11.0), &t.title, FontId::proportional(15.0), title_color, total);
        let sub = if t.album.is_empty() { t.artists.clone() } else { format!("{} • {}", t.artists, t.album) };
        paint_text(ui, pos2(tx, rect.min.y + 33.0), &sub, FontId::proportional(13.0), DIM, total);
    }
    resp.clicked()
}

/// Tres barras animadas (paradas quando pausado).
fn equalizer(ui: &egui::Ui, c: Pos2, playing: bool) {
    let t = if playing { ui.input(|i| i.time) as f32 } else { 0.0 };
    for (i, (speed, phase)) in [(5.0_f32, 0.0_f32), (6.5, 1.7), (4.2, 3.1)].into_iter().enumerate() {
        let h = 6.0 + 8.0 * (0.5 + 0.5 * (t * speed + phase).sin());
        let x = c.x + (i as f32 - 1.0) * 6.0;
        ui.painter().rect_filled(Rect::from_min_max(pos2(x - 1.6, c.y + 8.0 - h), pos2(x + 1.6, c.y + 8.0)), 1.0, Color32::WHITE);
    }
}

/// Capa + titulo + artista da faixa atual.
fn now_playing(ui: &mut egui::Ui, st: &crate::player::PlayerState) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
    match &st.current {
        Some(t) => paint_cover(ui, rect, &t.thumb, 6.0, 120),
        None => {
            ui.painter().rect_filled(rect, 6.0, Color32::from_gray(40));
        }
    }
    ui.vertical(|ui| {
        let (title, sub) = match (&st.current, &st.status) {
            (_, Status::Error(e)) => ("Não foi possível tocar".to_string(), e.clone()),
            (Some(t), Status::Loading) => (t.title.clone(), "Carregando...".to_string()),
            (Some(t), _) if t.album.is_empty() => (t.title.clone(), t.artists.clone()),
            (Some(t), _) => (t.title.clone(), format!("{} • {}", t.artists, t.album)),
            (None, _) => ("Nada tocando".to_string(), "Busque uma música".to_string()),
        };
        ui.add(egui::Label::new(RichText::new(title).strong().size(15.0)).truncate());
        ui.add(egui::Label::new(RichText::new(sub).color(DIM)).truncate());
    });
}

struct BarOut {
    frac: f32,
    /// true quando o usuario soltou/clicou (valor final); false enquanto arrasta.
    committed: bool,
}

/// Barra fina arrastavel (progresso e volume). Engorda e mostra a bolinha no hover.
fn thin_bar(ui: &mut egui::Ui, width: f32, frac: f32, enabled: bool, color: Color32) -> Option<BarOut> {
    let sense = if enabled { Sense::click_and_drag() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(width, 14.0), sense);
    let active = enabled && (resp.hovered() || resp.dragged());
    let h = if active { 5.0 } else { 3.0 };
    let y = rect.center().y;
    let track = Rect::from_min_max(pos2(rect.left(), y - h / 2.0), pos2(rect.right(), y + h / 2.0));
    ui.painter().rect_filled(track, 2.0, Color32::from_white_alpha(45));
    let x = rect.left() + rect.width() * frac.clamp(0.0, 1.0);
    if enabled && x > rect.left() {
        ui.painter().rect_filled(Rect::from_min_max(track.min, pos2(x, track.max.y)), 2.0, color);
    }
    if active {
        ui.painter().circle_filled(pos2(x, y), 6.0, color);
    }
    if !enabled || !(resp.dragged() || resp.drag_stopped() || resp.clicked()) {
        return None;
    }
    let p = resp.interact_pointer_pos().or_else(|| ui.input(|i| i.pointer.latest_pos()))?;
    Some(BarOut { frac: ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0), committed: !resp.dragged() })
}

#[derive(Clone, Copy)]
enum Icon {
    Play,
    Pause,
    Prev,
    Next,
    Repeat,
    RepeatOne,
    Shuffle,
    Volume,
    VolumeMuted,
}

fn icon_button(ui: &mut egui::Ui, icon: Icon, size: f32) -> egui::Response {
    icon_button_color(ui, icon, size, Color32::from_gray(225))
}

/// Seta pequena (triangulo) apontando para a direita (`dir` = 1) ou esquerda (`dir` = -1).
fn arrow_head(p: &egui::Painter, tip: Pos2, dir: f32, color: Color32) {
    p.add(egui::Shape::convex_polygon(
        vec![tip + vec2(-dir * 4.5, -3.6), tip + vec2(-dir * 4.5, 3.6), tip + vec2(dir * 0.5, 0.0)],
        color,
        Stroke::NONE,
    ));
}

/// Arco de circulo como linha (angulos em radianos, 0 = direita).
fn arc(p: &egui::Painter, c: Pos2, r: f32, a0: f32, a1: f32, stroke: Stroke) {
    let pts: Vec<Pos2> = (0..=8)
        .map(|i| {
            let a = a0 + (a1 - a0) * i as f32 / 8.0;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(egui::Shape::line(pts, stroke));
}

fn speaker(p: &egui::Painter, c: Pos2, s: f32, color: Color32) {
    p.rect_filled(Rect::from_center_size(c + vec2(-s * 1.1, 0.0), vec2(s * 0.9, s * 1.2)), 1.0, color);
    p.add(egui::Shape::convex_polygon(
        vec![c + vec2(-s * 0.7, -s * 0.6), c + vec2(s * 0.5, -s * 1.4), c + vec2(s * 0.5, s * 1.4), c + vec2(-s * 0.7, s * 0.6)],
        color,
        Stroke::NONE,
    ));
}

/// Botao de icone desenhado com formas (nao depende de fontes de simbolos).
/// `base` e a cor em repouso; no hover fica branco.
fn icon_button_color(ui: &mut egui::Ui, icon: Icon, size: f32, base: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    let hot = resp.hovered();
    if hot {
        ui.painter().circle_filled(rect.center(), size / 2.0, Color32::from_white_alpha(18));
    }
    let color = if hot { Color32::WHITE } else { base };
    let c = rect.center();
    let s = size * 0.20;
    let p = ui.painter();
    let tri = |x: f32, dir: f32| {
        vec![pos2(x - dir * s, c.y - s * 1.2), pos2(x - dir * s, c.y + s * 1.2), pos2(x + dir * s, c.y)]
    };
    let stroke = Stroke::new(1.8, color);
    match icon {
        Icon::Repeat | Icon::RepeatOne => {
            // Laco retangular com cantos chanfrados e setas em cantos opostos.
            let (w, h, k) = (s * 1.7, s * 1.1, 2.5);
            let pts = vec![
                c + vec2(-w + k, -h),
                c + vec2(w - k, -h),
                c + vec2(w, -h + k),
                c + vec2(w, h - k),
                c + vec2(w - k, h),
                c + vec2(-w + k, h),
                c + vec2(-w, h - k),
                c + vec2(-w, -h + k),
            ];
            p.add(egui::Shape::closed_line(pts, stroke));
            arrow_head(p, c + vec2(w * 0.25, -h), 1.0, color);
            arrow_head(p, c + vec2(-w * 0.25, h), -1.0, color);
            if matches!(icon, Icon::RepeatOne) {
                p.text(c + vec2(0.0, 0.5), Align2::CENTER_CENTER, "1", FontId::proportional(s * 1.7), color);
            }
        }
        Icon::Shuffle => {
            let (w, h) = (s * 1.5, s * 1.0);
            p.line_segment([c + vec2(-w, -h), c + vec2(w - 3.0, h)], stroke);
            p.line_segment([c + vec2(-w, h), c + vec2(w - 3.0, -h)], stroke);
            arrow_head(p, c + vec2(w + 1.0, h), 1.0, color);
            arrow_head(p, c + vec2(w + 1.0, -h), 1.0, color);
        }
        Icon::Volume => {
            speaker(p, c + vec2(-s * 0.6, 0.0), s, color);
            arc(p, c + vec2(s * 0.2, 0.0), s * 1.1, -0.9, 0.9, stroke);
            arc(p, c + vec2(s * 0.2, 0.0), s * 1.9, -0.9, 0.9, stroke);
        }
        Icon::VolumeMuted => {
            speaker(p, c + vec2(-s * 0.9, 0.0), s, color);
            let o = c + vec2(s * 1.5, 0.0);
            p.line_segment([o + vec2(-s * 0.7, -s * 0.7), o + vec2(s * 0.7, s * 0.7)], stroke);
            p.line_segment([o + vec2(-s * 0.7, s * 0.7), o + vec2(s * 0.7, -s * 0.7)], stroke);
        }
        Icon::Play => {
            p.add(egui::Shape::convex_polygon(tri(c.x + s * 0.2, 1.0), color, Stroke::NONE));
        }
        Icon::Pause => {
            for dx in [-s * 0.7, s * 0.7] {
                p.rect_filled(Rect::from_center_size(pos2(c.x + dx, c.y), vec2(s * 0.7, s * 2.4)), 1.5, color);
            }
        }
        Icon::Next => {
            p.add(egui::Shape::convex_polygon(tri(c.x - s * 0.3, 1.0), color, Stroke::NONE));
            p.rect_filled(Rect::from_center_size(pos2(c.x + s * 1.1, c.y), vec2(s * 0.5, s * 2.4)), 1.0, color);
        }
        Icon::Prev => {
            let flipped = vec![pos2(c.x + s * 1.0, c.y - s * 1.2), pos2(c.x + s * 1.0, c.y + s * 1.2), pos2(c.x - s * 0.8, c.y)];
            p.add(egui::Shape::convex_polygon(flipped, color, Stroke::NONE));
            p.rect_filled(Rect::from_center_size(pos2(c.x - s * 1.4, c.y), vec2(s * 0.5, s * 2.4)), 1.0, color);
        }
    }
    resp
}

fn run_login_window() -> Result<Auth, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let login = exe.with_file_name("ytmusic-login.exe");
    if !login.is_file() {
        return Err("ytmusic-login.exe nao encontrado ao lado do app.".into());
    }
    let out = std::process::Command::new(login).output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().rev().find(|l| l.trim_start().starts_with('{')).ok_or("Login cancelado.")?;
    let v: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let cookie = v["cookie"].as_str().ok_or("Resposta de login invalida.")?;
    Auth::from_cookie_header(cookie).ok_or_else(|| "Login sem sessao valida (SAPISID ausente).".into())
}

fn setup_style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.window_corner_radius = 12.0.into();
    v.extreme_bg_color = Color32::from_rgb(0x2a, 0x2a, 0x2a);
    v.selection.bg_fill = Color32::from_white_alpha(60);
    v.hyperlink_color = ACCENT;
    v.widgets.inactive.weak_bg_fill = Color32::from_white_alpha(22);
    v.widgets.hovered.weak_bg_fill = Color32::from_white_alpha(38);
    v.widgets.active.weak_bg_fill = Color32::from_white_alpha(60);
    v.widgets.inactive.fg_stroke.color = Color32::from_gray(230);
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.button_padding = vec2(14.0, 5.0);
        s.spacing.interact_size.y = 28.0;
    });
}

/// Segoe UI como fonte principal + fallbacks do Windows para CJK/simbolos.
fn setup_fonts(ctx: &egui::Context) {
    use egui::{FontData, FontDefinitions, FontFamily};
    use std::sync::Arc;
    let mut fonts = FontDefinitions::default();
    let dir = std::path::Path::new(r"C:\Windows\Fonts");
    let candidates = [
        ("segoeui", "segoeui.ttf", true),
        ("segoesym", "seguisym.ttf", false),
        ("yugothic", "YuGothR.ttc", false),
        ("msyh", "msyh.ttc", false),
        ("malgun", "malgun.ttf", false),
    ];
    for (name, file, primary) in candidates {
        if let Ok(bytes) = std::fs::read(dir.join(file)) {
            fonts.font_data.insert(name.to_string(), Arc::new(FontData::from_owned(bytes)));
            for family in [FontFamily::Proportional, FontFamily::Monospace] {
                let list = fonts.families.entry(family).or_default();
                if primary {
                    list.insert(0, name.to_string());
                } else {
                    list.push(name.to_string());
                }
            }
        }
    }
    ctx.set_fonts(fonts);
}

#[cfg(test)]
mod tests {
    use super::shuffle_in_place;

    #[test]
    fn shuffle_keeps_all_items() {
        let mut v: Vec<u32> = (0..50).collect();
        shuffle_in_place(&mut v);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(sorted, (0..50).collect::<Vec<u32>>());
        let mut empty: Vec<u32> = Vec::new();
        shuffle_in_place(&mut empty);
    }
}
