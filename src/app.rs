//! Interface (egui) no estilo do YouTube Music: barra superior com busca,
//! menu lateral, listas/grades e barra do player com progresso fino.

use crate::auth::Auth;
use crate::mini::{self, MiniAction};
use crate::deps::{self, Component};
use crate::player::{Player, Status};
use crate::prefs::Prefs;
use crate::tr;
use crate::updater;
use crate::util::fmt_time;
use crate::ytm::{Client, Playlist, Track, thumb_url};
use eframe::egui::{self, Align, Align2, Color32, FontId, Layout, Margin, Pos2, Rect, RichText, Sense, Stroke, Vec2, pos2, vec2};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

mod settings;
mod update;

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
/// Largura minima para mostrar o painel "A seguir" a direita.
const QUEUE_AT: f32 = 960.0;
const QUEUE_W: f32 = 320.0;
const SEARCH_ID: &str = "search_box";

#[derive(Clone, Copy, PartialEq)]
enum QueueTab {
    UpNext,
    Related,
}

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
    /// Progresso do download de um componente (yt-dlp / Deno).
    DepsProgress(Component, u64, u64),
    DepsDone(Result<(), String>),
    Related(String, Result<Vec<Track>, String>),
    AddedToPlaylist(Playlist, Result<(), String>),
    /// Resultado de "Verificar atualizacoes" (None = ja esta na versao mais recente).
    UpdateChecked(Result<Option<updater::Release>, String>),
    UpdateProgress(u64, u64),
    UpdateDownloaded(Result<std::path::PathBuf, String>),
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
    /// Automix (mescla o fim de uma faixa com o inicio da proxima). Desligado por padrao.
    automix: bool,
    automix_secs: u32,
    /// Ultima "proxima faixa" informada ao player (so reenvia quando muda).
    sent_next: Option<String>,
    last_title: String,
    seek_drag: Option<f32>,

    repeat: Repeat,
    shuffle: bool,
    volume_before_mute: f32,

    notice: Option<String>,
    show_settings: bool,
    settings_page: settings::Page,
    update_state: update::UpdateState,
    cookie_input: String,
    account_name: Option<String>,
    /// Componentes que faltam para tocar (yt-dlp, runtime JS).
    missing: deps::Missing,
    deps_job: DepsJob,
    #[cfg(windows)]
    taskbar: Option<crate::taskbar::Taskbar>,
    #[cfg(windows)]
    media: Option<crate::media::MediaControls>,

    prefs: Prefs,
    /// Mini-player aberto agora.
    mini_visible: bool,
    /// O desfoque ja foi aplicado na janela atual do mini-player.
    mini_glass: bool,
    was_minimized: bool,
    /// Posicao atual do mini-player (salva nas preferencias ao fechar).
    mini_pos: Option<Pos2>,

    queue_tab: QueueTab,
    /// Sugestoes ("Relacionadas") da faixa atual, buscadas so quando o painel pede.
    related: Option<(String, Vec<Track>)>,
    related_pending: bool,
    /// Lista de playlists aberta no menu lateral.
    sidebar_playlists: bool,
    /// Aviso positivo que some sozinho (ex.: "Adicionada a ...").
    toast: Option<(String, Instant)>,
    /// Fontes CJK carregadas sob demanda: (japones/chines, coreano).
    cjk: (bool, bool),
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

/// Handle Win32 da janela principal.
#[cfg(windows)]
fn main_hwnd(cc: &eframe::CreationContext<'_>) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

/// Faixa que o automix pode mesclar depois de `queue[qi]`. Repetir faixa nunca mescla
/// (seria a mesma musica sobre ela mesma), nem uma fila de uma faixa so.
fn crossfade_target(queue: &[Track], qi: usize, repeat: Repeat) -> Option<&Track> {
    if repeat == Repeat::One {
        return None;
    }
    match queue.get(qi + 1) {
        Some(t) => Some(t),
        None if repeat == Repeat::All && queue.len() > 1 => queue.first(),
        None => None,
    }
}

/// Posicao na fila da faixa `id` que o automix acabou de comecar: a proxima ocorrencia
/// depois de `qi` ou, passando do fim (repetir fila), a primeira.
fn queue_index_of(queue: &[Track], qi: usize, id: &str) -> Option<usize> {
    queue
        .iter()
        .enumerate()
        .skip(qi + 1)
        .find(|(_, t)| t.video_id == id)
        .or_else(|| queue.iter().enumerate().find(|(_, t)| t.video_id == id))
        .map(|(i, _)| i)
}

/// Download dos componentes pelo app (botao "Baixar e instalar").
#[derive(Clone, PartialEq)]
enum DepsJob {
    Idle,
    Working(Component, u64, u64),
    Failed(String),
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
        setup_fonts(&cc.egui_ctx, (false, false));
        setup_style(&cc.egui_ctx);
        let (tx, rx) = channel();
        let client = Client::new(Auth::load());
        let (automix, automix_secs) = crate::player::load_automix();
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
            automix,
            automix_secs,
            sent_next: None,
            last_title: String::new(),
            seek_drag: None,
            repeat: Repeat::Off,
            shuffle: false,
            volume_before_mute: 0.5,
            notice: None,
            show_settings: false,
            settings_page: settings::Page::Account,
            update_state: update::UpdateState::Idle,
            cookie_input: String::new(),
            account_name: None,
            missing: deps::missing(),
            deps_job: DepsJob::Idle,
            #[cfg(windows)]
            taskbar: init_taskbar(cc),
            #[cfg(windows)]
            media: main_hwnd(cc).and_then(|h| crate::media::MediaControls::new(h, cc.egui_ctx.clone())),
            mini_pos: None,
            prefs: Prefs::load(),
            mini_visible: false,
            mini_glass: false,
            was_minimized: false,
            queue_tab: QueueTab::UpNext,
            related: None,
            related_pending: false,
            sidebar_playlists: false,
            toast: None,
            cjk: (false, false),
        };
        app.mini_pos = app.prefs.mini_pos.map(|p| pos2(p[0], p[1]));
        let lang = app.prefs.lang.as_deref().and_then(crate::i18n::Lang::from_code).unwrap_or_else(crate::i18n::Lang::system);
        crate::i18n::set_lang(lang);
        app.refresh_account();
        app.apply_automix(false);
        app
    }

    /// Aplica (e, se `save`, grava) a configuracao do automix no player.
    fn apply_automix(&self, save: bool) {
        if save {
            crate::player::save_automix(self.automix, self.automix_secs);
        }
        self.player.set_crossfade(if self.automix { self.automix_secs } else { 0 });
    }

    /// Faixa que a fila tocara depois da atual, se o automix puder mescla-la.
    /// Repetir faixa nao mescla (seria a mesma musica sobre ela mesma).
    fn crossfade_target(&self) -> Option<&Track> {
        crossfade_target(&self.queue, self.qi, self.repeat)
    }

    /// Mantem o player sabendo qual e a proxima faixa; so envia quando muda.
    fn sync_next_track(&mut self) {
        let target = if self.automix { self.crossfade_target().cloned() } else { None };
        let id = target.as_ref().map(|t| t.video_id.clone());
        if id != self.sent_next {
            self.player.set_next(target);
            self.sent_next = id;
        }
    }

    /// O automix ja esta tocando `current`: so acompanha a fila (nao toca de novo).
    fn follow_crossfade(&mut self, current: &Track) {
        if let Some(i) = queue_index_of(&self.queue, self.qi, &current.video_id) {
            self.qi = i;
        }
        self.consecutive_errors = 0;
        self.sent_next = None; // o player zerou a "proxima": manda a nova
        self.prefetch_next();
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
        self.notice = Some(tr!("login.opening").into());
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

    /// Carrega as fontes CJK (~14 MB cada) so quando aparece um texto que precisa delas.
    fn ensure_fonts<'a>(&mut self, ctx: &egui::Context, texts: impl Iterator<Item = &'a str>) {
        let mut need = self.cjk;
        for ch in texts.flat_map(str::chars) {
            match ch as u32 {
                0x3040..=0x30FF | 0x3400..=0x4DBF | 0x4E00..=0x9FFF | 0xF900..=0xFAFF | 0xFF66..=0xFF9F => need.0 = true,
                0x1100..=0x11FF | 0x3130..=0x318F | 0xAC00..=0xD7AF => need.1 = true,
                _ => {}
            }
            if need.0 && need.1 {
                break;
            }
        }
        if need != self.cjk {
            self.cjk = need;
            setup_fonts(ctx, need);
        }
    }

    fn ensure_fonts_for(&mut self, ctx: &egui::Context, tracks: &[Track]) {
        self.ensure_fonts(ctx, tracks.iter().flat_map(|t| [t.title.as_str(), t.artists.as_str(), t.album.as_str()]));
    }

    fn drain_messages(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match &msg {
                Msg::Search(Ok(t)) | Msg::Liked(Ok(t)) | Msg::PlaylistTracks(_, Ok(t)) | Msg::Mix(_, Ok(t)) | Msg::Related(_, Ok(t)) => {
                    self.ensure_fonts_for(ctx, t);
                }
                Msg::Playlists(Ok(list)) => {
                    self.ensure_fonts(ctx, list.iter().flat_map(|p| [p.title.as_str(), p.subtitle.as_str()]));
                }
                _ => {}
            }
            match msg {
                Msg::Search(r) => {
                    self.loading = false;
                    match r {
                        Ok(t) if t.is_empty() => self.notice = Some(tr!("search.nothing_found").into()),
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
                            self.notice = Some(tr!("account.connected_not_saved", error = e));
                        } else {
                            self.notice = Some(tr!("account.connected").into());
                        }
                        self.apply_auth(Some(auth));
                    }
                    Err(e) => self.notice = Some(e),
                },
                Msg::Related(for_id, r) => {
                    self.related_pending = false;
                    match r {
                        Ok(mut tracks) => {
                            tracks.retain(|t| t.video_id != for_id);
                            self.related = Some((for_id, tracks));
                        }
                        Err(e) => {
                            self.related = Some((for_id, Vec::new()));
                            self.notice = Some(e);
                        }
                    }
                }
                Msg::AddedToPlaylist(p, r) => match r {
                    Ok(()) => {
                        self.toast = Some((tr!("playlists.added", title = p.title), Instant::now()));
                        // A lista aberta (se for essa) ficou desatualizada.
                        if self.open_playlist.as_ref().is_some_and(|(o, _)| o.browse_id == p.browse_id) {
                            let id = p.browse_id.clone();
                            self.spawn(ctx, move |c| Msg::PlaylistTracks(p, c.playlist(&id)));
                        }
                    }
                    Err(e) => self.notice = Some(e),
                },
                Msg::DepsProgress(c, done, total) => self.deps_job = DepsJob::Working(c, done, total),
                Msg::DepsDone(result) => {
                    self.missing = deps::missing();
                    match result {
                        Ok(()) => {
                            self.deps_job = DepsJob::Idle;
                            self.notice = None;
                            self.toast = Some((tr!("deps.done").to_string(), Instant::now()));
                        }
                        Err(e) => self.deps_job = DepsJob::Failed(e),
                    }
                }
                Msg::UpdateChecked(result) => self.update_checked(result),
                Msg::UpdateProgress(done, total) => self.update_progress(done, total),
                Msg::UpdateDownloaded(result) => self.update_downloaded(result),
            }
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

    fn toggle_shuffle(&mut self) {
        self.shuffle = !self.shuffle;
        if self.shuffle {
            self.shuffle_upcoming();
        }
    }

    fn cycle_repeat(&mut self) {
        self.repeat = match self.repeat {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        };
    }

    /// Fila infinita a partir da faixa atual.
    fn start_mix(&mut self, ctx: &egui::Context) {
        if let Some(cur) = self.player.snapshot().current {
            self.queue = vec![cur];
            self.qi = 0;
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
        self.trim_image_memory(ctx);
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
            // Direita: [spinner] [mini-player] [engrenagem] [avatar].
            let right_w = 34.0 * 3.0 + 24.0 + ui.spacing().item_spacing.x * 4.0;
            let avail = (ui.available_width() - right_w).max(120.0);
            let search_w = avail.min(560.0);
            ui.add_space(((avail - search_w) / 2.0).max(0.0));
            self.search_box(ui, search_w);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.avatar(ui);
                self.gear(ui);
                self.mini_button(ui);
                if self.loading {
                    ui.spinner();
                }
            });
        });
        if !wide {
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                for (tab, label) in NAV {
                    if chip(ui, tr!(label), self.tab == tab) {
                        self.switch_tab(tab, &ui.ctx().clone());
                    }
                }
            });
        }
        ui.add_space(10.0);
    }

    fn search_box(&mut self, ui: &mut egui::Ui, width: f32) {
        const H: f32 = 40.0;
        egui::Frame::new().fill(SURFACE).corner_radius(H / 2.0).inner_margin(Margin::symmetric(14, 0)).show(ui, |ui| {
            ui.set_width((width - 28.0).max(60.0));
            ui.set_height(H);
            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::splat(18.0), Sense::hover());
                nav_icon(ui.painter(), NavIcon::Search, rect.center(), DIM);
                // Altura igual a da caixa e texto alinhado ao meio (antes a dica ficava alta).
                let resp = ui.add(
                    egui::TextEdit::singleline(&mut self.query)
                        .id(egui::Id::unique(SEARCH_ID))
                        .hint_text(tr!("search.hint"))
                        .font(FontId::proportional(15.0))
                        .desired_width(f32::INFINITY)
                        .frame(egui::Frame::NONE)
                        .margin(Margin::ZERO)
                        .min_size(vec2(0.0, H))
                        .align(Align2::LEFT_CENTER),
                );
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.search(&ui.ctx().clone());
                }
            });
        });
    }

    /// Botao "mini-player": minimiza a janela principal e abre o mini-player flutuante.
    fn mini_button(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
        let c = rect.center();
        if resp.hovered() {
            ui.painter().circle_filled(c, 17.0, Color32::from_white_alpha(22));
        }
        let color = if resp.hovered() { Color32::WHITE } else { Color32::from_gray(210) };
        let p = ui.painter();
        // Janela grande com uma janelinha preenchida no canto inferior direito.
        let outer = Rect::from_center_size(c, vec2(18.0, 14.0));
        p.rect_stroke(outer, 2.0, Stroke::new(1.8, color), egui::StrokeKind::Middle);
        let inner = Rect::from_min_max(outer.center() + vec2(0.5, 0.5), outer.max - vec2(2.5, 2.5));
        p.rect_filled(inner, 1.0, color);
        if resp.on_hover_text(tr!("mini.open")).clicked() {
            self.open_mini(&ui.ctx().clone());
        }
    }

    fn open_mini(&mut self, ctx: &egui::Context) {
        self.mini_visible = true;
        self.mini_glass = false;
        ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(true));
    }

    /// Engrenagem: abre as configuracoes do app (separadas da conta).
    fn gear(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
        let c = rect.center();
        let hot = resp.hovered() || (self.show_settings && self.settings_page != settings::Page::Account);
        if hot {
            ui.painter().circle_filled(c, 17.0, Color32::from_white_alpha(22));
        }
        let color = if hot { Color32::WHITE } else { Color32::from_gray(210) };
        let p = ui.painter();
        // Dentes: 8 tracos curtos e grossos em volta de um anel.
        for i in 0..8 {
            let a = i as f32 * std::f32::consts::TAU / 8.0;
            let d = vec2(a.cos(), a.sin());
            p.line_segment([c + d * 6.5, c + d * 9.5], Stroke::new(3.2, color));
        }
        p.circle_stroke(c, 6.2, Stroke::new(2.4, color));
        p.circle_filled(c, 2.2, color);
        if resp.on_hover_text(tr!("settings.tooltip")).clicked() {
            self.open_settings(settings::Page::Playback);
        }
    }

    fn avatar(&mut self, ui: &mut egui::Ui) {
        let (rect, resp) = ui.allocate_exact_size(Vec2::splat(34.0), Sense::click());
        let logged = self.client.is_logged_in();
        let c = rect.center();
        let fill = if logged { AVATAR } else { Color32::from_gray(70) };
        ui.painter().circle_filled(c, 17.0, fill);
        if resp.hovered() || (self.show_settings && self.settings_page == settings::Page::Account) {
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
        if resp.on_hover_text(tr!("account.tooltip")).clicked() {
            self.open_settings(settings::Page::Account);
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        ui.add_space(6.0);
        for (tab, label) in NAV {
            let icon = match tab {
                Tab::Search => NavIcon::Search,
                Tab::Liked => NavIcon::Heart,
                Tab::Playlists => NavIcon::List,
            };
            if nav_item(ui, tr!(label), icon, self.tab == tab) {
                if tab == Tab::Playlists {
                    // Clicar em Playlists abre/fecha a lista logo abaixo.
                    self.sidebar_playlists = self.tab != Tab::Playlists || self.open_playlist.is_some() || !self.sidebar_playlists;
                }
                self.switch_tab(tab, &ctx);
            }
        }

        // Arrastando uma musica: a lista abre sozinha para servir de alvo.
        let dragging = egui::DragAndDrop::payload::<Track>(&ctx).is_some();
        if !(self.sidebar_playlists || dragging) || !self.client.is_logged_in() {
            return;
        }
        self.ensure_playlists(&ctx);
        ui.add_space(8.0);
        ui.separator();
        ui.add_space(4.0);
        let hint = if dragging { tr!("playlists.drop_hint") } else { tr!("playlists.sidebar_title") };
        ui.label(RichText::new(hint).color(if dragging { Color32::WHITE } else { DIM }).size(12.0));
        ui.add_space(4.0);
        let Some(list) = self.playlists.clone() else {
            ui.spinner();
            return;
        };
        // So playlists que podem receber musicas (fora "Musicas curtidas" e afins).
        let list: Vec<Playlist> = list.into_iter().filter(|p| !matches!(p.browse_id.as_str(), "VLLM" | "VLSE")).collect();
        let mut open = None;
        let mut drop = None;
        ui.spacing_mut().item_spacing.y = 0.0;
        egui::ScrollArea::vertical().id_salt("side_playlists").auto_shrink([false, false]).show_rows(ui, 44.0, list.len(), |ui, range| {
            for p in &list[range] {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
                let target = resp.dnd_hover_payload::<Track>().is_some();
                if target {
                    ui.painter().rect_filled(rect, 8.0, Color32::from_white_alpha(40));
                    ui.painter().rect_stroke(rect.shrink(1.0), 8.0, Stroke::new(1.5, ACCENT), egui::StrokeKind::Inside);
                } else if resp.hovered() || self.open_playlist.as_ref().is_some_and(|(o, _)| o.browse_id == p.browse_id) {
                    ui.painter().rect_filled(rect, 8.0, Color32::from_white_alpha(14));
                }
                let img = Rect::from_min_size(rect.min + vec2(8.0, 6.0), Vec2::splat(32.0));
                paint_cover(ui, img, &p.thumb, 4.0, 60);
                paint_text(ui, pos2(img.right() + 10.0, rect.center().y - 9.0), &p.title, FontId::proportional(14.0), Color32::WHITE, rect.right() - img.right() - 18.0);
                if let Some(track) = resp.dnd_release_payload::<Track>() {
                    drop = Some((p.clone(), (*track).clone()));
                } else if resp.clicked() {
                    open = Some(p.clone());
                }
            }
        });
        if let Some((p, track)) = drop {
            let vid = track.video_id.clone();
            let id = p.browse_id.clone();
            self.spawn(&ctx, move |c| {
                let r = c.add_to_playlist(&id, &vid);
                Msg::AddedToPlaylist(p, r)
            });
        }
        if let Some(p) = open {
            self.tab = Tab::Playlists;
            self.loading = true;
            let id = p.browse_id.clone();
            self.spawn(&ctx, move |c| Msg::PlaylistTracks(p, c.playlist(&id)));
        }
    }

    /// Busca a lista de playlists uma vez (lazy: so quando algo precisa dela).
    fn ensure_playlists(&mut self, ctx: &egui::Context) {
        if self.playlists.is_none() && !self.loading && self.client.is_logged_in() {
            self.loading = true;
            self.spawn(ctx, |c| Msg::Playlists(c.library_playlists()));
        }
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
                let resp = track_row(ui, t, is_current, playing, show_album);
                if resp.drag_started() {
                    // Arrastar para uma playlist do menu lateral adiciona a musica nela.
                    resp.dnd_set_drag_payload(t.clone());
                } else if resp.clicked() {
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
                    empty_state(ui, tr!("search.empty_title"), tr!("search.empty_body"));
                } else {
                    let title = if self.last_query.is_empty() { tr!("search.results").to_string() } else { tr!("search.results_for", query = self.last_query) };
                    page_header(ui, &title, &tr!("common.n_songs", n = self.results.len()), wide, false);
                    let list = self.results.clone();
                    self.track_list(ui, &list, true);
                }
            }
            Tab::Liked => {
                if !self.client.is_logged_in() {
                    return self.needs_login(ui);
                }
                match self.liked.clone() {
                    Some(list) if list.is_empty() => empty_state(ui, tr!("liked.empty_title"), tr!("liked.empty_body")),
                    Some(list) => {
                        if page_header(ui, tr!("liked.title"), &tr!("common.n_songs", n = list.len()), wide, true) {
                            let ctx = ui.ctx().clone();
                            self.play_from(&list, 0, false, &ctx);
                        }
                        self.track_list(ui, &list, false);
                    }
                    None => empty_state(ui, tr!("common.loading"), ""),
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
                        Some(list) if list.is_empty() => empty_state(ui, tr!("playlists.empty_title"), tr!("playlists.empty_body")),
                        Some(list) => {
                            page_header(ui, tr!("playlists.title"), &tr!("playlists.in_library", n = list.len()), wide, false);
                            self.playlist_grid(ui, &list);
                        }
                        None => empty_state(ui, tr!("common.loading"), ""),
                    }
                }
            }
        }
    }

    fn playlist_header(&mut self, ui: &mut egui::Ui, p: &Playlist, tracks: &[Track], wide: bool) {
        ui.add_space(10.0);
        if chip(ui, tr!("common.back"), false) {
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
                let sub = if p.subtitle.is_empty() { tr!("common.n_songs", n = tracks.len()) } else { tr!("playlists.subtitle_with_count", subtitle = p.subtitle, n = tracks.len()) };
                ui.add(egui::Label::new(RichText::new(sub).color(DIM)).truncate());
                ui.add_space(10.0);
                play = pill_button(ui, tr!("common.play"), true).clicked();
            });
        });
        ui.add_space(12.0);
        if play {
            let ctx = ui.ctx().clone();
            self.play_from(tracks, 0, false, &ctx);
        }
    }

    fn needs_login(&mut self, ui: &mut egui::Ui) {
        empty_state(ui, tr!("login.needed_title"), tr!("login.needed_body"));
        ui.vertical_centered(|ui| {
            if pill_button(ui, tr!("login.connect"), true).clicked() {
                self.connect_account(&ui.ctx().clone());
            }
        });
    }

    /// Baixa o que falta (yt-dlp / Deno) em segundo plano. So roda pelo botao.
    fn install_deps(&mut self, ctx: &egui::Context) {
        if matches!(self.deps_job, DepsJob::Working(..)) {
            return;
        }
        let first = if self.missing.ytdlp { Component::YtDlp } else { Component::Deno };
        self.deps_job = DepsJob::Working(first, 0, 0);
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let (ptx, pctx) = (tx.clone(), ctx.clone());
            let result = deps::install_missing(move |c, done, total| {
                let _ = ptx.send(Msg::DepsProgress(c, done, total));
                pctx.request_repaint();
            });
            let _ = tx.send(Msg::DepsDone(result));
            ctx.request_repaint();
        });
    }

    /// Botao "Baixar e instalar" / barra de progresso / erro do download de componentes.
    pub(super) fn deps_controls(&mut self, ui: &mut egui::Ui) {
        match self.deps_job.clone() {
            DepsJob::Working(c, done, total) => {
                ui.vertical(|ui| {
                    ui.label(RichText::new(tr!("deps.downloading", name = c.label())).color(Color32::from_gray(230)));
                    let f = if total > 0 { (done as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
                    ui.add(egui::ProgressBar::new(f).desired_width(220.0).fill(ACCENT).show_percentage());
                });
            }
            DepsJob::Failed(_) => {
                if pill_button(ui, tr!("deps.retry"), true).clicked() {
                    self.install_deps(&ui.ctx().clone());
                }
            }
            DepsJob::Idle => {
                if pill_button(ui, tr!("deps.install"), true).clicked() {
                    self.install_deps(&ui.ctx().clone());
                }
            }
        }
    }

    /// Aviso no topo enquanto faltar algum componente para tocar.
    fn deps_bar(&mut self, ui: &mut egui::Ui) {
        if !self.missing.any() && !matches!(self.deps_job, DepsJob::Working(..)) {
            return;
        }
        let text = match (self.missing.ytdlp, self.missing.js_runtime) {
            (true, true) => tr!("deps.need_ytdlp_js"),
            (true, false) => tr!("deps.need_ytdlp"),
            _ => tr!("deps.need_js"),
        };
        egui::Frame::new().fill(Color32::from_rgb(0x2e, 0x16, 0x16)).corner_radius(8.0).inner_margin(Margin::symmetric(14, 10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.set_max_width((ui.available_width() - 250.0).max(200.0));
                    ui.add(egui::Label::new(RichText::new(text).strong().color(Color32::from_rgb(0xff, 0xc4, 0xbe))).wrap());
                    ui.label(RichText::new(tr!("deps.source_note")).color(DIM).small());
                    if let DepsJob::Failed(e) = &self.deps_job {
                        ui.add(egui::Label::new(RichText::new(tr!("deps.failed", error = e)).color(Color32::from_rgb(0xff, 0x9a, 0x92)).small()).wrap());
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| self.deps_controls(ui));
            });
        });
        ui.add_space(6.0);
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

    // ------------------------------------------------------------ queue panel

    /// Painel a direita no estilo do YouTube Music: "A seguir" (a fila) e
    /// "Relacionadas" (sugestoes da faixa atual, buscadas so quando abertas).
    fn queue_panel(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let st = self.player.snapshot();
        let current_id = st.current.as_ref().map(|t| t.video_id.clone());
        let playing = st.status == Status::Playing;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            for (tab, label) in [(QueueTab::UpNext, tr!("queue.up_next")), (QueueTab::Related, tr!("queue.related"))] {
                if chip(ui, label, self.queue_tab == tab) {
                    self.queue_tab = tab;
                }
            }
        });
        ui.add_space(8.0);
        ui.spacing_mut().item_spacing.y = 0.0;
        match self.queue_tab {
            QueueTab::UpNext => {
                if self.queue.is_empty() {
                    ui.label(RichText::new(tr!("queue.empty")).color(DIM));
                    return;
                }
                let start = self.qi.min(self.queue.len() - 1);
                let upcoming = self.queue[start..].to_vec();
                let mut jump = None;
                if upcoming.len() <= 1 {
                    ui.label(RichText::new(tr!("queue.end")).color(DIM).small());
                    ui.add_space(6.0);
                }
                egui::ScrollArea::vertical().id_salt("queue").auto_shrink([false, false]).show_rows(ui, 56.0, upcoming.len(), |ui, range| {
                    for i in range {
                        let t = &upcoming[i];
                        let resp = queue_row(ui, t, i == 0, playing);
                        if resp.drag_started() {
                            resp.dnd_set_drag_payload(t.clone());
                        } else if resp.clicked() && i > 0 {
                            jump = Some(start + i);
                        }
                    }
                });
                if let Some(i) = jump {
                    self.qi = i;
                    self.start_current();
                }
            }
            QueueTab::Related => {
                let Some(cur) = current_id else {
                    ui.label(RichText::new(tr!("queue.related_needs_track")).color(DIM));
                    return;
                };
                let fresh = self.related.as_ref().is_some_and(|(id, _)| *id == cur);
                if !fresh && !self.related_pending {
                    self.related_pending = true;
                    let id = cur.clone();
                    self.spawn(&ctx, move |c| {
                        let r = c.mix(&id);
                        Msg::Related(id, r)
                    });
                }
                let list = match &self.related {
                    Some((id, list)) if *id == cur => list.clone(),
                    _ => {
                        ui.spinner();
                        return;
                    }
                };
                let mut pick = None;
                egui::ScrollArea::vertical().id_salt("related").auto_shrink([false, false]).show_rows(ui, 56.0, list.len(), |ui, range| {
                    for i in range {
                        let resp = queue_row(ui, &list[i], false, playing);
                        if resp.drag_started() {
                            resp.dnd_set_drag_payload(list[i].clone());
                        } else if resp.clicked() {
                            pick = Some(list[i].clone());
                        }
                    }
                });
                if let Some(t) = pick {
                    // Toca ja e mantem o resto da fila depois dela.
                    self.queue.retain(|q| q.video_id != t.video_id);
                    let cur_idx = self.queue.iter().position(|q| q.video_id == cur);
                    let at = cur_idx.map(|i| i + 1).unwrap_or(0).min(self.queue.len());
                    self.queue.insert(at, t);
                    self.qi = at;
                    self.start_current();
                }
            }
        }
    }

    // -------------------------------------------------------------- shortcuts

    /// Atalhos de teclado (ignorados enquanto se digita na busca).
    fn shortcuts(&mut self, ctx: &egui::Context, st: &crate::player::PlayerState) {
        use egui::{Key, Modifiers};
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::F)) {
            ctx.memory_mut(|m| m.request_focus(egui::Id::unique(SEARCH_ID)));
            return;
        }
        if ctx.egui_wants_keyboard_input() {
            return;
        }
        let pressed = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        if pressed(Modifiers::NONE, Key::Slash) {
            ctx.memory_mut(|m| m.request_focus(egui::Id::unique(SEARCH_ID)));
        }
        if pressed(Modifiers::NONE, Key::Space) || pressed(Modifiers::NONE, Key::K) {
            self.toggle_playback(st);
        }
        if pressed(Modifiers::COMMAND, Key::ArrowRight) || pressed(Modifiers::SHIFT, Key::N) {
            self.next(ctx);
        }
        if pressed(Modifiers::COMMAND, Key::ArrowLeft) || pressed(Modifiers::SHIFT, Key::P) {
            self.prev();
        }
        if st.current.is_some() {
            let step = Duration::from_secs(10);
            if pressed(Modifiers::NONE, Key::ArrowRight) || pressed(Modifiers::NONE, Key::L) {
                self.player.seek((st.position + step).min(st.duration));
            }
            if pressed(Modifiers::NONE, Key::ArrowLeft) || pressed(Modifiers::NONE, Key::J) {
                self.player.seek(st.position.saturating_sub(step));
            }
        }
        if pressed(Modifiers::NONE, Key::ArrowUp) {
            self.player.set_volume(st.volume + 0.05);
        }
        if pressed(Modifiers::NONE, Key::ArrowDown) {
            self.player.set_volume(st.volume - 0.05);
        }
        if pressed(Modifiers::NONE, Key::M) {
            if st.volume < 0.01 {
                self.player.set_volume(self.volume_before_mute.max(0.3));
            } else {
                self.volume_before_mute = st.volume;
                self.player.set_volume(0.0);
            }
        }
        if pressed(Modifiers::NONE, Key::S) {
            self.toggle_shuffle();
        }
        if pressed(Modifiers::NONE, Key::R) {
            self.cycle_repeat();
        }
    }

    // ------------------------------------------------------------ mini-player

    /// Desenha o mini-player (janela flutuante) quando ele esta aberto.
    fn draw_mini(&mut self, ctx: &egui::Context, st: &crate::player::PlayerState) {
        if !self.mini_visible {
            return;
        }
        let pos = self.mini_pos.or_else(|| {
            // Primeira vez: canto inferior direito, acima da barra de tarefas.
            let monitor = ctx.input(|i| i.viewport().monitor_size)?;
            let size = mini::size(self.prefs.mini_compact);
            Some(pos2(monitor.x - size.x - 24.0, monitor.y - size.y - 72.0))
        });
        let view = mini::MiniView {
            pinned: self.prefs.mini_pinned,
            compact: self.prefs.mini_compact,
            shuffle: self.shuffle,
            repeat: match self.repeat {
                Repeat::Off => 0,
                Repeat::All => 1,
                Repeat::One => 2,
            },
        };
        let id = egui::ViewportId::from_hash_of("mini_player");
        let (actions, outer, close_req) = ctx.show_viewport_immediate(id, mini::viewport_builder(pos, view.compact), |ui, _class| {
            let actions = mini::ui(ui, st, view);
            let (outer, close) = ui.ctx().input(|i| (i.viewport().outer_rect, i.viewport().close_requested()));
            (actions, outer, close)
        });
        if !self.mini_glass {
            // A janela so existe depois do primeiro quadro; tenta ate achar.
            self.mini_glass = mini::apply_glass();
            ctx.request_repaint_after(Duration::from_millis(100));
        }
        if let Some(r) = outer {
            self.mini_pos = Some(r.min);
        }
        if close_req {
            self.close_mini();
        }
        for a in actions {
            match a {
                MiniAction::Prev => self.prev(),
                MiniAction::Toggle => self.toggle_playback(st),
                MiniAction::Next => self.next(ctx),
                MiniAction::Seek(f) => self.player.seek(st.duration.mul_f32(f)),
                MiniAction::Shuffle => self.toggle_shuffle(),
                MiniAction::Repeat => self.cycle_repeat(),
                MiniAction::Mix => self.start_mix(ctx),
                MiniAction::TogglePin => {
                    self.prefs.mini_pinned = !self.prefs.mini_pinned;
                    self.save_prefs();
                }
                MiniAction::ToggleCompact => {
                    // Mantem o canto superior direito no lugar ao trocar de tamanho.
                    let (old, new) = (mini::size(self.prefs.mini_compact), mini::size(!self.prefs.mini_compact));
                    if let Some(p) = self.mini_pos.as_mut() {
                        p.x += old.x - new.x;
                    }
                    self.prefs.mini_compact = !self.prefs.mini_compact;
                    if let Some(p) = self.mini_pos {
                        ctx.send_viewport_cmd_to(id, egui::ViewportCommand::OuterPosition(p));
                    }
                    self.save_prefs();
                }
                MiniAction::Hide => self.close_mini(),
                MiniAction::Close => {
                    self.close_mini();
                    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
                }
            }
        }
    }

    fn close_mini(&mut self) {
        self.mini_visible = false;
        self.save_prefs();
    }

    fn save_prefs(&mut self) {
        self.prefs.mini_pos = self.mini_pos.map(|p| [p.x, p.y]);
        self.prefs.save();
    }

    /// Libera capas decodificadas quando o cache de imagens cresce demais
    /// (as visiveis sao recarregadas sob demanda).
    fn trim_image_memory(&self, ctx: &egui::Context) {
        let loaders = ctx.loaders();
        let bytes: usize = loaders.bytes.lock().iter().map(|l| l.byte_size()).sum::<usize>()
            + loaders.image.lock().iter().map(|l| l.byte_size()).sum::<usize>();
        if bytes > 48 * 1024 * 1024 {
            ctx.forget_all_images();
        }
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

        let q_color = if self.prefs.show_queue { Color32::WHITE } else { DIM };
        if icon_button_color(ui, Icon::Queue, 38.0, q_color).on_hover_text(tr!("queue.toggle_tooltip")).clicked() {
            self.prefs.show_queue = !self.prefs.show_queue;
            self.save_prefs();
        }

        let resp = ui.add_enabled(st.current.is_some(), egui::Button::new(RichText::new(tr!("player.mix")).strong()).corner_radius(16.0).min_size(vec2(52.0, 30.0)));
        if resp.on_hover_text(tr!("player.mix_tooltip")).clicked() {
            self.start_mix(ctx);
        }

        let shuffle_color = if self.shuffle { Color32::WHITE } else { DIM };
        let shuffle_tip = if self.shuffle { tr!("player.shuffle_on") } else { tr!("player.shuffle_off") };
        if icon_button_color(ui, Icon::Shuffle, 38.0, shuffle_color).on_hover_text(shuffle_tip).clicked() {
            self.toggle_shuffle();
        }

        let (repeat_icon, repeat_color, repeat_tip) = match self.repeat {
            Repeat::Off => (Icon::Repeat, DIM, tr!("player.repeat_off")),
            Repeat::All => (Icon::Repeat, Color32::WHITE, tr!("player.repeat_all")),
            Repeat::One => (Icon::RepeatOne, Color32::WHITE, tr!("player.repeat_one")),
        };
        if icon_button_color(ui, repeat_icon, 38.0, repeat_color).on_hover_text(repeat_tip).clicked() {
            self.cycle_repeat();
        }

        if with_volume {
            ui.add_space(8.0);
            if let Some(o) = thin_bar(ui, 96.0, st.volume, true, Color32::WHITE) {
                self.player.set_volume(o.frac);
            }
            let muted = st.volume < 0.01;
            let vol_icon = if muted { Icon::VolumeMuted } else { Icon::Volume };
            if icon_button_color(ui, vol_icon, 38.0, Color32::from_gray(225)).on_hover_text(tr!("player.mute")).clicked() {
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
    /// Roda sempre, inclusive com a janela minimizada (quando o eframe nao chama
    /// `ui`): fila, teclas de midia, botoes da barra de tarefas e o mini-player.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_messages(ctx);

        // O automix comecou a proxima sozinho: a fila so acompanha.
        let mut st = self.player.snapshot();
        if st.advanced {
            self.player.state.lock().unwrap().advanced = false;
            if let Some(current) = st.current.clone() {
                self.follow_crossfade(&current);
            }
            st.advanced = false;
        }
        self.sync_next_track();

        // Avanca a fila quando a faixa termina (ou falha).
        if st.finished {
            self.player.state.lock().unwrap().finished = false;
            if let Status::Error(msg) = &st.status {
                self.missing = deps::missing();
                if self.missing.ytdlp {
                    // Sem o yt-dlp nenhuma faixa toca: nao pula a fila; o aviso de
                    // componentes (com o botao de instalar) ja explica.
                    self.consecutive_errors = 0;
                } else {
                    self.consecutive_errors += 1;
                    self.notice = Some(msg.clone());
                    if self.consecutive_errors < 3 {
                        self.next(ctx);
                    }
                }
            } else {
                self.consecutive_errors = 0;
                if self.repeat == Repeat::One {
                    self.start_current();
                } else {
                    self.next(ctx);
                }
            }
        }
        let st = self.player.snapshot();
        // Botoes da miniatura na barra de tarefas (anterior / play-pause / proxima).
        #[cfg(windows)]
        {
            let playing = st.status == Status::Playing;
            let actions = self.taskbar.as_mut().map(|tb| tb.poll(playing)).unwrap_or_default();
            for action in actions {
                match action {
                    crate::taskbar::Action::Prev => self.prev(),
                    crate::taskbar::Action::Toggle => self.toggle_playback(&st),
                    crate::taskbar::Action::Next => self.next(ctx),
                }
            }
        }
        // Teclas de midia / painel de midia do Windows.
        #[cfg(windows)]
        {
            let playing = st.status == Status::Playing;
            let actions = self.media.as_mut().map(|m| m.poll(st.current.as_ref(), playing)).unwrap_or_default();
            for action in actions {
                use crate::media::MediaAction;
                match action {
                    MediaAction::Play if st.status != Status::Playing => self.toggle_playback(&st),
                    MediaAction::Pause if st.status == Status::Playing => self.player.toggle(),
                    MediaAction::Next => self.next(ctx),
                    MediaAction::Prev => self.prev(),
                    _ => {}
                }
            }
        }

        // Mini-player: so abre pelo botao da barra superior (minimizar continua
        // sendo so minimizar); fecha quando a janela principal volta.
        let minimized = ctx.input(|i| i.viewport().minimized).unwrap_or(false);
        if minimized != self.was_minimized {
            self.was_minimized = minimized;
            if !minimized && self.mini_visible {
                self.close_mini();
            }
        }
        if minimized && self.mini_visible {
            // Com a janela principal minimizada o eframe so roda esta logica; isto
            // pede um quadro completo para desenhar o mini-player.
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::RequestPaintWhileHidden);
            ctx.request_repaint();
        }

        // Titulo da janela (e da barra de tarefas) acompanha a faixa.
        let title = match &st.current {
            Some(t) if st.status == Status::Playing => format!("{} - {}", t.artists, t.title),
            _ => tr!("app.name").to_string(),
        };
        if title != self.last_title {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }

        // Redesenho economico: so enquanto toca/carrega, e mais devagar quando a
        // janela principal nao esta a vista (o mini-player so mostra o tempo).
        if matches!(st.status, Status::Playing | Status::Loading) {
            let unfocused = ctx.input(|i| !i.focused);
            let every = if minimized { 1000 } else if unfocused { 500 } else { 250 };
            ctx.request_repaint_after(Duration::from_millis(every));
        }
        if let Some((_, at)) = &self.toast {
            if at.elapsed() > Duration::from_secs(3) {
                self.toast = None;
            } else {
                ctx.request_repaint_after(Duration::from_millis(500));
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        let wide = ui.available_width() >= WIDE_AT;
        let st = self.player.snapshot();
        self.draw_mini(&ctx, &st);
        if self.was_minimized {
            // Nada da janela principal aparece: pula o layout dela (CPU/GPU).
            return;
        }
        self.shortcuts(&ctx, &st);

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
        let show_queue = self.prefs.show_queue && ui.available_width() + if wide { 210.0 } else { 0.0 } >= QUEUE_AT && st.current.is_some();
        if show_queue {
            egui::Panel::right("queue")
                .resizable(false)
                .exact_size(QUEUE_W)
                .frame(egui::Frame::new().fill(BG).inner_margin(Margin { left: 8, right: 12, top: 0, bottom: 0 }))
                .show(ui, |ui| self.queue_panel(ui));
        }
        let content_margin = Margin::symmetric(if wide { 24 } else { 12 }, 0);
        egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(content_margin)).show(ui, |ui| {
            self.deps_bar(ui);
            self.notice_bar(ui);
            self.content(ui, wide);
        });
        self.settings_window(&ctx);

        // Musica sendo arrastada: etiqueta seguindo o mouse.
        if let Some(t) = egui::DragAndDrop::payload::<Track>(&ctx) {
            if let Some(p) = ctx.pointer_interact_pos() {
                let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::unique("drag_ghost")));
                let text = format!("♪  {}", t.title);
                let galley = painter.layout_no_wrap(text, FontId::proportional(14.0), Color32::WHITE);
                let rect = Rect::from_min_size(p + vec2(14.0, 8.0), galley.size() + vec2(20.0, 12.0));
                painter.rect_filled(rect, 8.0, Color32::from_rgba_unmultiplied(40, 40, 40, 235));
                painter.rect_stroke(rect, 8.0, Stroke::new(1.0, Color32::from_white_alpha(50)), egui::StrokeKind::Inside);
                painter.galley(rect.min + vec2(10.0, 6.0), galley, Color32::WHITE);
            }
        }

        // Aviso positivo (some em 3 s).
        if let Some((msg, _)) = &self.toast {
            egui::Area::new(egui::Id::unique("toast"))
                .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -(PLAYER_H + 36.0)))
                .order(egui::Order::Foreground)
                .interactable(false)
                .show(&ctx, |ui| {
                    egui::Frame::new().fill(Color32::from_rgb(0xf1, 0xf1, 0xf1)).corner_radius(8.0).inner_margin(Margin::symmetric(16, 10)).show(ui, |ui| {
                        ui.label(RichText::new(msg).color(Color32::BLACK));
                    });
                });
        }
    }

    /// Fundo opaco da janela principal (ela e criada "transparente" so para o
    /// mini-player poder ter fundo de vidro).
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        BG.to_normalized_gamma_f32()
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.save_prefs();
    }
}

/// Linha compacta do painel "A seguir".
fn queue_row(ui: &mut egui::Ui, t: &Track, active: bool, playing: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::click_and_drag());
    paint_row_bg(ui, rect, &resp, active);
    let img = Rect::from_min_size(rect.min + vec2(6.0, 8.0), Vec2::splat(40.0));
    paint_cover(ui, img, &t.thumb, 4.0, 60);
    if active {
        ui.painter().rect_filled(img, 4.0, Color32::from_black_alpha(130));
        equalizer(ui, img.center(), playing);
    }
    let tx = img.right() + 10.0;
    let dur = if t.duration_secs > 0 { fmt_time(t.duration_secs as u64) } else { String::new() };
    ui.painter().text(pos2(rect.right() - 6.0, rect.center().y), Align2::RIGHT_CENTER, &dur, FontId::proportional(12.0), DIM);
    let w = rect.right() - tx - 46.0;
    paint_text(ui, pos2(tx, rect.min.y + 10.0), &t.title, FontId::proportional(14.0), Color32::WHITE, w);
    paint_text(ui, pos2(tx, rect.min.y + 30.0), &t.artists, FontId::proportional(12.0), DIM, w);
    resp
}

// ------------------------------------------------------------------- helpers

/// Abas e a chave de traducao do rotulo.
const NAV: [(Tab, &str); 3] = [(Tab::Search, "nav.search"), (Tab::Liked, "nav.liked"), (Tab::Playlists, "nav.playlists")];

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
                play = pill_button(ui, tr!("common.play"), true).clicked();
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

pub(crate) fn paint_cover(ui: &egui::Ui, rect: Rect, thumb: &str, radius: f32, size: u32) {
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
pub(crate) fn paint_text(ui: &egui::Ui, pos: Pos2, text: &str, font: FontId, color: Color32, max_w: f32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap.max_width = max_w.max(10.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(pos, galley, color);
}

fn track_row(ui: &mut egui::Ui, t: &Track, is_current: bool, playing: bool, show_album: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
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
    resp
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

/// Capa + titulo + artista da faixa atual, centralizados na vertical.
fn now_playing(ui: &mut egui::Ui, st: &crate::player::PlayerState) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::hover());
    let cover = Rect::from_min_size(pos2(rect.min.x, rect.center().y - 22.0), Vec2::splat(44.0));
    match &st.current {
        Some(t) => paint_cover(ui, cover, &t.thumb, 6.0, 120),
        None => {
            ui.painter().rect_filled(cover, 6.0, Color32::from_gray(40));
        }
    }
    let (title, sub) = match (&st.current, &st.status) {
        (_, Status::Error(e)) => (tr!("player.cannot_play").to_string(), e.clone()),
        (Some(t), Status::Loading) if st.downloading => (t.title.clone(), tr!("player.downloading").to_string()),
        (Some(t), Status::Loading) => (t.title.clone(), tr!("common.loading").to_string()),
        (Some(t), _) if t.album.is_empty() => (t.title.clone(), t.artists.clone()),
        (Some(t), _) => (t.title.clone(), format!("{} • {}", t.artists, t.album)),
        (None, _) => (tr!("player.nothing_playing").to_string(), tr!("player.search_hint").to_string()),
    };
    let x = cover.right() + 12.0;
    let w = (rect.right() - x).max(20.0);
    let cy = rect.center().y;
    paint_text(ui, pos2(x, cy - 20.0), &title, FontId::proportional(15.0), Color32::WHITE, w);
    paint_text(ui, pos2(x, cy + 1.0), &sub, FontId::proportional(13.0), DIM, w);
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
pub(crate) enum Icon {
    Play,
    Pause,
    Prev,
    Next,
    Repeat,
    RepeatOne,
    Shuffle,
    Volume,
    VolumeMuted,
    Queue,
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
pub(crate) fn icon_button_color(ui: &mut egui::Ui, icon: Icon, size: f32, base: Color32) -> egui::Response {
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
        Icon::Queue => {
            for i in 0..3 {
                let y = c.y - s * 1.0 + i as f32 * s;
                p.line_segment([pos2(c.x - s * 1.4, y), pos2(c.x + s * 0.6, y)], stroke);
            }
            p.add(egui::Shape::convex_polygon(
                vec![pos2(c.x + s * 1.0, c.y + s * 0.3), pos2(c.x + s * 1.0, c.y + s * 1.5), pos2(c.x + s * 1.8, c.y + s * 0.9)],
                color,
                Stroke::NONE,
            ));
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
        return Err(tr!("login.helper_missing").into());
    }
    let out = std::process::Command::new(login).output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let line = stdout.lines().rev().find(|l| l.trim_start().starts_with('{')).ok_or(tr!("login.cancelled"))?;
    let v: serde_json::Value = serde_json::from_str(line).map_err(|e| e.to_string())?;
    let cookie = v["cookie"].as_str().ok_or(tr!("login.invalid_response"))?;
    Auth::from_cookie_header(cookie).ok_or_else(|| tr!("login.no_session").into())
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

/// Segoe UI como fonte principal + simbolos. As fontes CJK (japones/chines e
/// coreano, ~14 MB cada) so entram quando algum texto precisa delas.
fn setup_fonts(ctx: &egui::Context, cjk: (bool, bool)) {
    use egui::{FontData, FontDefinitions, FontFamily};
    use std::sync::Arc;
    let mut fonts = FontDefinitions::default();
    let dir = std::path::Path::new(r"C:\Windows\Fonts");
    let candidates = [
        ("segoeui", "segoeui.ttf", true, true),
        ("segoesym", "seguisym.ttf", false, true),
        ("yugothic", "YuGothR.ttc", false, cjk.0),
        ("malgun", "malgun.ttf", false, cjk.1),
    ];
    for (name, file, primary, wanted) in candidates {
        if !wanted {
            continue;
        }
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
    use super::{Repeat, crossfade_target, queue_index_of, shuffle_in_place};
    use crate::ytm::Track;

    fn queue(ids: &[&str]) -> Vec<Track> {
        ids.iter().map(|id| Track { video_id: id.to_string(), ..Default::default() }).collect()
    }

    fn id(t: Option<&Track>) -> Option<&str> {
        t.map(|t| t.video_id.as_str())
    }

    #[test]
    fn crossfade_target_follows_queue_and_repeat_rules() {
        let q = queue(&["a", "b", "c"]);
        assert_eq!(id(crossfade_target(&q, 0, Repeat::Off)), Some("b"));
        assert_eq!(id(crossfade_target(&q, 1, Repeat::All)), Some("c"));
        // Fim da fila: sem repeticao nao ha o que mesclar; repetir fila volta ao inicio.
        assert_eq!(id(crossfade_target(&q, 2, Repeat::Off)), None);
        assert_eq!(id(crossfade_target(&q, 2, Repeat::All)), Some("a"));
        // Repetir faixa: nunca mescla; fila de uma faixa so com repetir fila tambem nao.
        assert_eq!(id(crossfade_target(&q, 0, Repeat::One)), None);
        assert_eq!(id(crossfade_target(&queue(&["a"]), 0, Repeat::All)), None);
        assert_eq!(id(crossfade_target(&[], 0, Repeat::Off)), None);
    }

    #[test]
    fn queue_follows_the_track_the_automix_started() {
        let q = queue(&["a", "b", "c"]);
        assert_eq!(queue_index_of(&q, 0, "b"), Some(1));
        assert_eq!(queue_index_of(&q, 2, "a"), Some(0)); // volta ao inicio (repetir fila)
        // Faixa repetida na fila: pega a proxima ocorrencia depois da atual, nao a primeira.
        let dup = queue(&["a", "b", "a", "c"]);
        assert_eq!(queue_index_of(&dup, 1, "a"), Some(2));
        assert_eq!(queue_index_of(&q, 0, "zzz"), None);
    }

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
