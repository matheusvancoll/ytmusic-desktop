//! Botoes de controle na miniatura da barra de tarefas do Windows (anterior,
//! play/pause, proxima), como o Spotify. Usa a "thumbnail toolbar" do Windows
//! (`ITaskbarList3`). Os cliques chegam como `WM_COMMAND` na janela; um subclass
//! os converte em [`Action`], que o app le a cada quadro com [`Taskbar::poll`].

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Action {
    Prev,
    Toggle,
    Next,
}

/// Glifos desenhados em codigo (sem arquivos de imagem).
#[derive(Clone, Copy)]
pub enum Glyph {
    Prev,
    Play,
    Pause,
    Next,
}

pub const ICON_PX: usize = 32;

/// Desenha o glifo em RGBA (reto, nao pre-multiplicado), `ICON_PX` x `ICON_PX`.
/// Branco com um contorno escuro fino, para aparecer em tema claro e escuro.
pub fn render_glyph(glyph: Glyph) -> Vec<u8> {
    const SS: usize = 4; // super-amostragem para suavizar as bordas
    let n = ICON_PX * SS;
    let tri = |x: f32, y: f32, a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        let s = |p: (f32, f32), q: (f32, f32)| (x - q.0) * (p.1 - q.1) - (p.0 - q.0) * (y - q.1);
        let (d1, d2, d3) = (s(a, b), s(b, c), s(c, a));
        !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
    };
    let rect = |x: f32, y: f32, x0: f32, x1: f32| x >= x0 && x <= x1 && (0.20..=0.80).contains(&y);
    let inside = |x: f32, y: f32| match glyph {
        Glyph::Play => tri(x, y, (0.30, 0.20), (0.30, 0.80), (0.78, 0.50)),
        Glyph::Pause => rect(x, y, 0.26, 0.44) || rect(x, y, 0.56, 0.74),
        Glyph::Next => tri(x, y, (0.20, 0.20), (0.20, 0.80), (0.64, 0.50)) || rect(x, y, 0.68, 0.80),
        Glyph::Prev => tri(x, y, (0.80, 0.20), (0.80, 0.80), (0.36, 0.50)) || rect(x, y, 0.20, 0.32),
    };
    // Mascara em alta resolucao.
    let mut mask = vec![false; n * n];
    for py in 0..n {
        for px in 0..n {
            mask[py * n + px] = inside((px as f32 + 0.5) / n as f32, (py as f32 + 0.5) / n as f32);
        }
    }
    // Dilatacao de ~1 pixel final para o contorno.
    let r = SS as isize;
    let mut dil = vec![false; n * n];
    for py in 0..n as isize {
        for px in 0..n as isize {
            'o: for dy in -r..=r {
                for dx in -r..=r {
                    let (x, y) = (px + dx, py + dy);
                    if x >= 0 && y >= 0 && (x as usize) < n && (y as usize) < n && mask[y as usize * n + x as usize] {
                        dil[py as usize * n + px as usize] = true;
                        break 'o;
                    }
                }
            }
        }
    }
    let mut out = vec![0u8; ICON_PX * ICON_PX * 4];
    for y in 0..ICON_PX {
        for x in 0..ICON_PX {
            let (mut fg, mut bg) = (0usize, 0usize);
            for sy in 0..SS {
                for sx in 0..SS {
                    let i = (y * SS + sy) * n + x * SS + sx;
                    fg += mask[i] as usize;
                    bg += dil[i] as usize;
                }
            }
            let a_w = fg as f32 / (SS * SS) as f32;
            let a_b = (bg as f32 / (SS * SS) as f32) * 0.65;
            let a = a_w + a_b * (1.0 - a_w);
            if a > 0.0 {
                let c = (255.0 * a_w) / a; // branco sobre preto
                let o = (y * ICON_PX + x) * 4;
                out[o] = c as u8;
                out[o + 1] = c as u8;
                out[o + 2] = c as u8;
                out[o + 3] = (a * 255.0) as u8;
            }
        }
    }
    out
}

#[cfg(windows)]
pub use imp::Taskbar;

#[cfg(windows)]
mod imp {
    use super::{Action, Glyph, ICON_PX, render_glyph};
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows::Win32::Graphics::Gdi::{CreateBitmap, DeleteObject, HGDIOBJ};
    use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx};
    use windows::Win32::UI::Shell::{
        DefSubclassProc, ITaskbarList3, SetWindowSubclass, TaskbarList, THB_FLAGS, THB_ICON, THB_TOOLTIP, THBF_ENABLED, THBN_CLICKED,
        THUMBBUTTON,
    };
    use windows::Win32::UI::WindowsAndMessaging::{CreateIconIndirect, DestroyIcon, HICON, ICONINFO, RegisterWindowMessageW, WM_COMMAND, WM_SIZE};
    use windows::core::w;

    const ID_PREV: u32 = 1;
    const ID_TOGGLE: u32 = 2;
    const ID_NEXT: u32 = 3;

    /// Cliques pendentes (um bit por acao) deixados pelo subclass para o app ler.
    static PENDING: AtomicU32 = AtomicU32::new(0);
    /// O Windows avisa (de novo) quando o botao da barra de tarefas e criado/recriado.
    static BUTTON_CREATED: AtomicBool = AtomicBool::new(false);
    static BUTTON_CREATED_MSG: AtomicU32 = AtomicU32::new(0);
    static CTX: OnceLock<eframe::egui::Context> = OnceLock::new();

    unsafe extern "system" fn subclass_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM, _id: usize, _data: usize) -> LRESULT {
        if msg == WM_COMMAND && ((wparam.0 >> 16) & 0xffff) as u32 == THBN_CLICKED {
            let bit = match (wparam.0 & 0xffff) as u32 {
                ID_PREV => 1,
                ID_TOGGLE => 2,
                ID_NEXT => 4,
                _ => 0,
            };
            if bit != 0 {
                PENDING.fetch_or(bit, Ordering::SeqCst);
                if let Some(ctx) = CTX.get() {
                    ctx.request_repaint(); // acorda o app mesmo com a janela em segundo plano
                }
                return LRESULT(0);
            }
        }
        // Minimizar/restaurar nao gera um quadro no eframe; acordamos o app para
        // ele abrir/fechar o mini-player na hora.
        if msg == WM_SIZE {
            if let Some(ctx) = CTX.get() {
                ctx.request_repaint();
            }
        }
        let created = BUTTON_CREATED_MSG.load(Ordering::Relaxed);
        if created != 0 && msg == created {
            BUTTON_CREATED.store(true, Ordering::SeqCst);
            if let Some(ctx) = CTX.get() {
                ctx.request_repaint();
            }
        }
        unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
    }

    fn make_icon(glyph: Glyph) -> Option<HICON> {
        let rgba = render_glyph(glyph);
        // O bitmap de cor e BGRA.
        let mut bgra = rgba.clone();
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        let mask = vec![0u8; ICON_PX * ICON_PX / 8];
        unsafe {
            let color = CreateBitmap(ICON_PX as i32, ICON_PX as i32, 1, 32, Some(bgra.as_ptr().cast()));
            let mono = CreateBitmap(ICON_PX as i32, ICON_PX as i32, 1, 1, Some(mask.as_ptr().cast()));
            let info = ICONINFO { fIcon: true.into(), xHotspot: 0, yHotspot: 0, hbmMask: mono, hbmColor: color };
            let icon = CreateIconIndirect(&info).ok();
            let _ = DeleteObject(HGDIOBJ(color.0));
            let _ = DeleteObject(HGDIOBJ(mono.0));
            icon
        }
    }

    fn tip(text: &str) -> [u16; 260] {
        let mut buf = [0u16; 260];
        for (i, u) in text.encode_utf16().take(259).enumerate() {
            buf[i] = u;
        }
        buf
    }

    pub struct Taskbar {
        hwnd: HWND,
        list: ITaskbarList3,
        icons: [HICON; 4], // prev, play, pause, next
        /// Os botoes ja foram registrados com sucesso.
        added: bool,
        /// Ha um registro a fazer (o Windows avisou, ou o plano B venceu) e ele ainda nao deu certo.
        pending: bool,
        /// O aviso "TaskbarButtonCreated" ja chegou pelo menos uma vez.
        seen_created: bool,
        playing: bool,
        created_at: Instant,
        last_try: Option<Instant>,
        tries: u32,
    }

    /// Se o aviso do Windows nunca chegar (ele pode ter sido enviado antes de instalarmos o
    /// subclass), registramos os botoes depois desse tempo.
    const FALLBACK_AFTER: Duration = Duration::from_secs(3);

    impl Taskbar {
        /// `hwnd` e o handle Win32 da janela principal.
        pub fn new(hwnd: isize, ctx: eframe::egui::Context) -> Option<Taskbar> {
            let hwnd = HWND(hwnd as *mut _);
            let _ = CTX.set(ctx);
            let list: ITaskbarList3 = unsafe {
                // O winit ja costuma inicializar o COM; ignorar o resultado e seguro.
                let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
                let list: ITaskbarList3 = CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER).ok()?;
                list.HrInit().ok()?;
                BUTTON_CREATED_MSG.store(RegisterWindowMessageW(w!("TaskbarButtonCreated")), Ordering::Relaxed);
                if !SetWindowSubclass(hwnd, Some(subclass_proc), 0xB0DE, 0).as_bool() {
                    return None;
                }
                list
            };
            let icons = [make_icon(Glyph::Prev)?, make_icon(Glyph::Play)?, make_icon(Glyph::Pause)?, make_icon(Glyph::Next)?];
            Some(Taskbar {
                hwnd,
                list,
                icons,
                added: false,
                pending: false,
                seen_created: false,
                playing: false,
                created_at: Instant::now(),
                last_try: None,
                tries: 0,
            })
        }

        fn buttons(&self, playing: bool) -> [THUMBBUTTON; 3] {
            let mk = |id: u32, icon: HICON, text: &str| THUMBBUTTON {
                dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
                iId: id,
                iBitmap: 0,
                hIcon: icon,
                szTip: tip(text),
                dwFlags: THBF_ENABLED,
            };
            let (toggle_icon, toggle_tip) = if playing { (self.icons[2], "Pausar") } else { (self.icons[1], "Reproduzir") };
            [mk(ID_PREV, self.icons[0], "Anterior"), mk(ID_TOGGLE, toggle_icon, toggle_tip), mk(ID_NEXT, self.icons[3], "Próxima")]
        }

        /// Chame uma vez por quadro. Registra os botoes (com tentativas, porque o
        /// botao da barra de tarefas pode demorar a existir), troca o icone de
        /// play/pause e devolve os cliques recebidos desde a ultima chamada.
        pub fn poll(&mut self, playing: bool) -> Vec<Action> {
            let wake = |after: Duration| {
                if let Some(ctx) = CTX.get() {
                    ctx.request_repaint_after(after);
                }
            };

            // Metodo documentado: registrar quando o Windows avisa que o botao da barra de
            // tarefas existe (e de novo se o Explorer o recriar, porque os botoes se perdem).
            if BUTTON_CREATED.swap(false, Ordering::SeqCst) {
                self.seen_created = true;
                self.added = false;
                self.pending = true;
                self.tries = 0;
                self.last_try = None;
            } else if !self.added && !self.pending && !self.seen_created {
                // Plano B: o aviso pode ter sido enviado antes do nosso subclass existir.
                if self.created_at.elapsed() >= FALLBACK_AFTER {
                    self.pending = true;
                } else {
                    wake(Duration::from_millis(500));
                }
            }

            if self.pending && self.tries < 80 {
                if self.last_try.is_none_or(|t| t.elapsed() >= Duration::from_millis(250)) {
                    self.last_try = Some(Instant::now());
                    self.tries += 1;
                    let buttons = self.buttons(playing);
                    match unsafe { self.list.ThumbBarAddButtons(self.hwnd, &buttons) } {
                        Ok(()) => {
                            self.added = true;
                            self.pending = false;
                            self.playing = playing;
                            #[cfg(debug_assertions)]
                            eprintln!("taskbar: botoes registrados (tentativa {}, aviso do Windows: {})", self.tries, self.seen_created);
                        }
                        Err(_e) => {
                            #[cfg(debug_assertions)]
                            eprintln!("taskbar: tentativa {} falhou: {_e}", self.tries);
                            wake(Duration::from_millis(250));
                        }
                    }
                } else {
                    wake(Duration::from_millis(250));
                }
            } else if self.added && playing != self.playing {
                self.playing = playing;
                let buttons = self.buttons(playing);
                let _ = unsafe { self.list.ThumbBarUpdateButtons(self.hwnd, &buttons) };
            }

            let bits = PENDING.swap(0, Ordering::SeqCst);
            let mut actions = Vec::new();
            for (bit, action) in [(1, Action::Prev), (2, Action::Toggle), (4, Action::Next)] {
                if bits & bit != 0 {
                    #[cfg(debug_assertions)]
                    eprintln!("taskbar: clique {action:?}");
                    actions.push(action);
                }
            }
            actions
        }
    }

    impl Drop for Taskbar {
        fn drop(&mut self) {
            for icon in self.icons {
                let _ = unsafe { DestroyIcon(icon) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha(px: &[u8], x: usize, y: usize) -> u8 {
        px[(y * ICON_PX + x) * 4 + 3]
    }

    #[test]
    fn glyphs_have_expected_size_and_shape() {
        for g in [Glyph::Prev, Glyph::Play, Glyph::Pause, Glyph::Next] {
            let px = render_glyph(g);
            assert_eq!(px.len(), ICON_PX * ICON_PX * 4);
            assert_eq!(alpha(&px, 0, 0), 0, "canto deve ser transparente");
            assert!(px.chunks_exact(4).any(|p| p[3] == 255), "deve ter pixels opacos");
        }
    }

    #[test]
    fn play_is_filled_at_center_and_pause_has_a_gap() {
        let c = ICON_PX / 2;
        let play = render_glyph(Glyph::Play);
        assert_eq!(alpha(&play, c, c), 255);
        // Pause: o miolo (entre as duas barras) so tem, no maximo, contorno escuro.
        let pause = render_glyph(Glyph::Pause);
        let mid = (pause[(c * ICON_PX + c) * 4], pause[(c * ICON_PX + c) * 4 + 3]);
        assert!(mid.1 < 255 || mid.0 < 255, "o centro do pause nao pode ser branco opaco");
        let left_bar = &pause[(c * ICON_PX + 10) * 4..(c * ICON_PX + 10) * 4 + 4];
        assert_eq!(left_bar, [255, 255, 255, 255]);
    }
}
