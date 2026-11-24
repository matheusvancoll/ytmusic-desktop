//! Mini-player flutuante: janela pequena, sem bordas, sempre por cima de tudo,
//! com fundo de vidro (desfoque do Windows + tinta translucida). Dois formatos:
//!
//! - **Completo**: capa, titulo, controles, aleatorio / repetir / Mix e progresso.
//! - **Compacto**: so anterior / play-pause / proxima e uma barrinha de progresso.
//!
//! Os botoes de trocar formato / travar / esconder / fechar so aparecem com o
//! mouse em cima. Destravado, arrasta por qualquer ponto do fundo.
//!
//! E desenhado como viewport "imediato" do egui: roda dentro do quadro da janela
//! principal (o app pede um quadro mesmo minimizado enquanto o mini-player esta
//! aberto), entao nao precisa de estado compartilhado nem de outro thread.

use crate::app::{Icon, icon_button_color, paint_cover, paint_text};
use crate::player::{PlayerState, Status};
use crate::tr;
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, Vec2, pos2, vec2};

pub const SIZE_FULL: Vec2 = vec2(360.0, 128.0);
pub const SIZE_COMPACT: Vec2 = vec2(232.0, 50.0);
pub const TITLE: &str = "YT Music - mini-player";
const RADIUS: f32 = 8.0;
const RED: Color32 = Color32::from_rgb(0xff, 0x1e, 0x1e);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MiniAction {
    Prev,
    Toggle,
    Next,
    Seek(f32),
    Shuffle,
    Repeat,
    Mix,
    TogglePin,
    ToggleCompact,
    /// Esconde o mini-player (o app continua minimizado).
    Hide,
    /// Fecha o mini-player e volta para a janela principal.
    Close,
}

/// O que o mini-player precisa saber alem do estado do player.
#[derive(Clone, Copy)]
pub struct MiniView {
    pub pinned: bool,
    pub compact: bool,
    pub shuffle: bool,
    /// 0 = desligado, 1 = fila, 2 = faixa.
    pub repeat: u8,
}

pub fn size(compact: bool) -> Vec2 {
    if compact { SIZE_COMPACT } else { SIZE_FULL }
}

pub fn viewport_builder(pos: Option<Pos2>, compact: bool) -> egui::ViewportBuilder {
    let size = size(compact);
    let mut b = egui::ViewportBuilder::default()
        .with_title(TITLE)
        .with_inner_size(size)
        .with_min_inner_size(size)
        .with_max_inner_size(size)
        .with_resizable(false)
        .with_decorations(false)
        .with_transparent(true)
        .with_taskbar(false)
        .with_window_level(egui::WindowLevel::AlwaysOnTop);
    if let Some(p) = pos {
        b = b.with_position(p);
    }
    b
}

/// Desenha o mini-player e devolve o que o usuario fez.
pub fn ui(ui: &mut egui::Ui, st: &PlayerState, view: MiniView) -> Vec<MiniAction> {
    let mut actions = Vec::new();
    let full = ui.max_rect();
    let ctx = ui.ctx().clone();
    let hovered = ctx.input(|i| i.pointer.hover_pos()).is_some_and(|p| full.contains(p));

    glass_background(ui, full);

    // Fundo inteiro: arrasta a janela quando destravado.
    let bg = ui.interact(full, ui.scope_id().with("mini_bg"), if view.pinned { Sense::hover() } else { Sense::click_and_drag() });
    if bg.drag_started() && !view.pinned {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }

    if view.compact {
        compact(ui, full, st, hovered, &mut actions);
    } else {
        complete(ui, full, st, view, hovered, &mut actions);
    }

    progress(ui, full, st, view.compact, &mut actions);

    if hovered {
        hover_buttons(ui, full, view, &mut actions);
    }
    actions
}

/// Tinta escura translucida sobre o desfoque do Windows, brilho sutil em cima e borda fina.
fn glass_background(ui: &egui::Ui, full: Rect) {
    let p = ui.painter();
    p.rect_filled(full, RADIUS, Color32::from_rgba_unmultiplied(18, 18, 22, 165));
    let shine = Rect::from_min_max(full.min, pos2(full.max.x, full.min.y + full.height() * 0.45));
    p.rect_filled(shine, egui::CornerRadius { nw: RADIUS as u8, ne: RADIUS as u8, sw: 0, se: 0 }, Color32::from_white_alpha(10));
    p.rect_stroke(full.shrink(0.5), RADIUS, Stroke::new(1.0, Color32::from_white_alpha(38)), egui::StrokeKind::Inside);
}

fn transport(ui: &mut egui::Ui, rect: Rect, st: &PlayerState, small: bool, actions: &mut Vec<MiniAction>) {
    let (side, main) = if small { (28.0, 30.0) } else { (32.0, 34.0) };
    let playing = st.status == Status::Playing;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.spacing_mut().item_spacing.x = 2.0;
    if icon_button_color(&mut child, Icon::Prev, side, Color32::from_gray(225)).clicked() {
        actions.push(MiniAction::Prev);
    }
    let icon = if playing { Icon::Pause } else { Icon::Play };
    if icon_button_color(&mut child, icon, main, Color32::WHITE).clicked() {
        actions.push(MiniAction::Toggle);
    }
    if icon_button_color(&mut child, Icon::Next, side, Color32::from_gray(225)).clicked() {
        actions.push(MiniAction::Next);
    }
}

fn time_text(st: &PlayerState) -> Option<String> {
    st.current.as_ref()?;
    let dur = st.duration.as_secs_f32().max(1.0);
    let pos = st.position.as_secs_f32().min(dur);
    Some(format!("{} / {}", crate::util::fmt_time(pos as u64), crate::util::fmt_time(dur as u64)))
}

/// Formato compacto: [⏮ ▶ ⏭] e o tempo (ou os botoes de janela, no hover).
fn compact(ui: &mut egui::Ui, full: Rect, st: &PlayerState, hovered: bool, actions: &mut Vec<MiniAction>) {
    let row = Rect::from_min_size(full.min + vec2(8.0, 6.0), vec2(96.0, 34.0));
    transport(ui, row, st, true, actions);
    if !hovered {
        if let Some(t) = time_text(st) {
            ui.painter().text(pos2(full.right() - 12.0, row.center().y), Align2::RIGHT_CENTER, t, FontId::proportional(11.0), Color32::from_gray(180));
        }
    }
}

/// Formato completo: capa, titulo/artista, controles e aleatorio / repetir / Mix.
fn complete(ui: &mut egui::Ui, full: Rect, st: &PlayerState, view: MiniView, hovered: bool, actions: &mut Vec<MiniAction>) {
    // Capa (duplo clique volta para o app).
    let cover = Rect::from_min_size(full.min + vec2(14.0, 14.0), Vec2::splat(72.0));
    match &st.current {
        Some(t) => paint_cover(ui, cover, &t.thumb, 6.0, 120),
        None => {
            ui.painter().rect_filled(cover, 6.0, Color32::from_white_alpha(20));
        }
    }
    if ui.interact(cover, ui.scope_id().with("mini_cover"), Sense::click()).double_clicked() {
        actions.push(MiniAction::Close);
    }

    // Titulo e artista (mais curtos quando os botoes do hover aparecem).
    let tx = cover.right() + 12.0;
    let text_w = full.right() - tx - if hovered { 104.0 } else { 14.0 };
    let (title, sub) = match (&st.current, &st.status) {
        (_, Status::Error(_)) => (tr!("player.cannot_play").to_string(), tr!("mini.try_next").to_string()),
        (Some(t), Status::Loading) if st.downloading => (t.title.clone(), tr!("mini.downloading").to_string()),
        (Some(t), _) => (t.title.clone(), t.artists.clone()),
        (None, _) => (tr!("player.nothing_playing").to_string(), tr!("app.name").to_string()),
    };
    paint_text(ui, pos2(tx, full.min.y + 14.0), &title, FontId::proportional(14.0), Color32::WHITE, text_w);
    paint_text(ui, pos2(tx, full.min.y + 33.0), &sub, FontId::proportional(12.0), Color32::from_gray(185), text_w);

    // Linha 1: transporte + tempo.
    let row1 = Rect::from_min_size(pos2(tx - 6.0, full.min.y + 52.0), vec2(110.0, 34.0));
    transport(ui, row1, st, false, actions);
    if let Some(t) = time_text(st) {
        ui.painter().text(pos2(full.right() - 14.0, row1.center().y), Align2::RIGHT_CENTER, t, FontId::proportional(11.0), Color32::from_gray(170));
    }

    // Linha 2: aleatorio, repetir, Mix.
    let row2 = Rect::from_min_size(pos2(tx - 4.0, full.min.y + 86.0), vec2(full.right() - tx - 10.0, 28.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(row2).layout(egui::Layout::left_to_right(egui::Align::Center)));
    child.spacing_mut().item_spacing.x = 4.0;
    let on = |b: bool| if b { Color32::WHITE } else { Color32::from_gray(140) };
    let shuffle_tip = if view.shuffle { tr!("player.shuffle_on") } else { tr!("player.shuffle_off") };
    if icon_button_color(&mut child, Icon::Shuffle, 28.0, on(view.shuffle)).on_hover_text(shuffle_tip).clicked() {
        actions.push(MiniAction::Shuffle);
    }
    let (repeat_icon, repeat_tip) = match view.repeat {
        0 => (Icon::Repeat, tr!("player.repeat_off")),
        1 => (Icon::Repeat, tr!("player.repeat_all")),
        _ => (Icon::RepeatOne, tr!("player.repeat_one")),
    };
    if icon_button_color(&mut child, repeat_icon, 28.0, on(view.repeat > 0)).on_hover_text(repeat_tip).clicked() {
        actions.push(MiniAction::Repeat);
    }
    let mix = egui::Button::new(egui::RichText::new(tr!("player.mix")).size(12.0).strong())
        .corner_radius(12.0)
        .fill(Color32::from_white_alpha(26))
        .min_size(vec2(42.0, 22.0));
    if child.add_enabled(st.current.is_some(), mix).on_hover_text(tr!("player.mix_tooltip")).clicked() {
        actions.push(MiniAction::Mix);
    }
}

/// Barra fina no rodape (clicavel para buscar no tempo).
fn progress(ui: &mut egui::Ui, full: Rect, st: &PlayerState, compact: bool, actions: &mut Vec<MiniAction>) {
    let inset = if compact { 10.0 } else { 14.0 };
    let bar = Rect::from_min_max(pos2(full.min.x + inset, full.max.y - 8.0), pos2(full.max.x - inset, full.max.y - 5.0));
    let resp = ui.interact(bar.expand2(vec2(0.0, 4.0)), ui.scope_id().with("mini_bar"), Sense::click());
    ui.painter().rect_filled(bar, 2.0, Color32::from_white_alpha(40));
    if st.current.is_none() {
        return;
    }
    let dur = st.duration.as_secs_f32().max(1.0);
    let frac = (st.position.as_secs_f32() / dur).clamp(0.0, 1.0);
    let x = bar.left() + bar.width() * frac;
    ui.painter().rect_filled(Rect::from_min_max(bar.min, pos2(x, bar.max.y)), 2.0, RED);
    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            actions.push(MiniAction::Seek(((p.x - bar.left()) / bar.width()).clamp(0.0, 1.0)));
        }
    }
}

#[derive(Clone, Copy)]
enum Hover {
    Size,
    Pin,
    Hide,
    Close,
}

/// Botoes que so aparecem no hover: [formato] [travar] [esconder] [fechar].
fn hover_buttons(ui: &mut egui::Ui, full: Rect, view: MiniView, actions: &mut Vec<MiniAction>) {
    let size = if view.compact { 20.0 } else { 22.0 };
    let y = if view.compact { full.center().y - size / 2.0 - 2.0 } else { full.min.y + 8.0 };
    let mut x = full.right() - 8.0 - size;
    let size_tip = if view.compact { tr!("mini.expand") } else { tr!("mini.shrink") };
    let pin_tip = if view.pinned { tr!("mini.unpin") } else { tr!("mini.pin") };
    let buttons = [(Hover::Close, tr!("mini.close")), (Hover::Hide, tr!("mini.hide")), (Hover::Pin, pin_tip), (Hover::Size, size_tip)];
    for (kind, tip) in buttons {
        let rect = Rect::from_min_size(pos2(x, y), Vec2::splat(size));
        let resp = ui.interact(rect, ui.scope_id().with(("mini_hover", tip)), Sense::click()).on_hover_text(tip);
        hover_button(ui.painter(), rect, kind, resp.hovered(), view);
        if resp.clicked() {
            actions.push(match kind {
                Hover::Close => MiniAction::Close,
                Hover::Hide => MiniAction::Hide,
                Hover::Pin => MiniAction::TogglePin,
                Hover::Size => MiniAction::ToggleCompact,
            });
        }
        x -= size + 4.0;
    }
}

fn hover_button(p: &egui::Painter, rect: Rect, kind: Hover, hot: bool, view: MiniView) {
    let c = rect.center();
    let fill = match kind {
        Hover::Close if hot => Color32::from_rgb(0xc4, 0x2b, 0x1c),
        Hover::Pin if view.pinned => Color32::from_white_alpha(70),
        _ if hot => Color32::from_white_alpha(46),
        _ => Color32::from_white_alpha(22),
    };
    p.circle_filled(c, rect.width() / 2.0, fill);
    let stroke = Stroke::new(1.5, Color32::WHITE);
    let k = rect.width() / 22.0; // escala dos desenhos
    match kind {
        Hover::Close => {
            let s = 4.0 * k;
            p.line_segment([c + vec2(-s, -s), c + vec2(s, s)], stroke);
            p.line_segment([c + vec2(-s, s), c + vec2(s, -s)], stroke);
        }
        Hover::Hide => {
            p.line_segment([c + vec2(-4.5 * k, 0.0), c + vec2(4.5 * k, 0.0)], stroke);
        }
        Hover::Pin => {
            // Alfinete: inclinado quando solto, reto (com base) quando travado.
            let dir = if view.pinned { vec2(0.0, 1.0) } else { vec2(0.6, 0.8) };
            let head = c - dir * 3.0 * k;
            p.circle_filled(head, 3.2 * k, Color32::WHITE);
            p.line_segment([head, head + dir * 8.5 * k], stroke);
            if view.pinned {
                p.line_segment([c + vec2(-4.0 * k, 0.5), c + vec2(4.0 * k, 0.5)], stroke);
            }
        }
        Hover::Size => {
            // Compacto: setas para fora (expandir). Completo: setas para dentro (encolher).
            let (a, b) = if view.compact { (1.5 * k, 4.5 * k) } else { (4.5 * k, 1.5 * k) };
            for d in [vec2(1.0, -1.0), vec2(-1.0, 1.0)] {
                let from = c + d * a;
                let to = c + d * b;
                p.line_segment([from, to], stroke);
                // ponta da seta em `to`
                let back = (from - to).normalized() * 2.5 * k;
                let perp = vec2(-back.y, back.x);
                p.line_segment([to, to + back + perp], stroke);
                p.line_segment([to, to + back - perp], stroke);
            }
        }
    }
}

/// Liga o desfoque do Windows atras da janela do mini-player e arredonda os
/// cantos (Windows 11). Devolve `true` quando achou a janela.
#[cfg(windows)]
pub fn apply_glass() -> bool {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DWM_WINDOW_CORNER_PREFERENCE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute};
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
    use windows::core::{HSTRING, s, w};

    #[repr(C)]
    struct AccentPolicy {
        state: u32,
        flags: u32,
        gradient: u32,
        animation: u32,
    }
    #[repr(C)]
    struct CompositionData {
        attrib: u32,
        data: *mut std::ffi::c_void,
        size: usize,
    }
    const WCA_ACCENT_POLICY: u32 = 19;
    const ACCENT_ENABLE_BLURBEHIND: u32 = 3;
    type SetWca = unsafe extern "system" fn(HWND, *mut CompositionData) -> i32;

    unsafe {
        let Ok(hwnd) = FindWindowW(None, &HSTRING::from(TITLE)) else { return false };
        if hwnd.0.is_null() {
            return false;
        }
        let pref = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&pref as *const DWM_WINDOW_CORNER_PREFERENCE).cast(),
            std::mem::size_of_val(&pref) as u32,
        );
        // SetWindowCompositionAttribute nao esta nos headers publicos; e o mesmo
        // recurso que o Explorer e o Terminal usam para o desfoque.
        if let Ok(user32) = GetModuleHandleW(w!("user32.dll")) {
            if let Some(f) = GetProcAddress(user32, s!("SetWindowCompositionAttribute")) {
                let set: SetWca = std::mem::transmute(f);
                let mut accent = AccentPolicy { state: ACCENT_ENABLE_BLURBEHIND, flags: 0, gradient: 0, animation: 0 };
                let mut data = CompositionData {
                    attrib: WCA_ACCENT_POLICY,
                    data: (&mut accent as *mut AccentPolicy).cast(),
                    size: std::mem::size_of::<AccentPolicy>(),
                };
                set(hwnd, &mut data);
            }
        }
        true
    }
}

#[cfg(not(windows))]
pub fn apply_glass() -> bool {
    true
}
