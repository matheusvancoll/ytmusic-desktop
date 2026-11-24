#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod auth;
mod deps;
mod i18n;
mod media;
mod mini;
mod player;
mod prefs;
mod taskbar;
mod updater;
mod util;
mod ytm;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();

    // Modo de depuracao: `ytmusic --search "termo"` imprime os resultados.
    // `ytmusic --liked` imprime as musicas curtidas da conta conectada.
    let debug = args.get(1).map(String::as_str);
    if matches!(debug, Some("--search") | Some("--liked")) {
        let client = ytm::Client::new(auth::Auth::load());
        let result = if debug == Some("--liked") { client.liked() } else { client.search(&args[2..].join(" ")) };
        match result {
            Ok(tracks) => tracks.iter().for_each(|t| {
                println!("{} | {} | {} | {}", t.video_id, t.title, t.artists, util::fmt_time(t.duration_secs as u64));
            }),
            Err(e) => eprintln!("erro: {e}"),
        }
        return Ok(());
    }

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("YT Music")
        .with_inner_size([1000.0, 680.0])
        .with_min_inner_size([360.0, 520.0])
        // Permite que a janela do mini-player (filha) tenha fundo translucido.
        .with_transparent(true);
    if let Some(icon) = app_icon() {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native("YT Music", options, Box::new(|cc| Ok(Box::new(app::App::new(cc)))))
}

/// Icone da janela (barra de titulo, barra de tarefas e Alt+Tab). Sem isso o
/// eframe usa o icone padrao do egui.
fn app_icon() -> Option<eframe::egui::IconData> {
    let img = image::load_from_memory(include_bytes!("../assets/icon.png")).ok()?.into_rgba8();
    let (width, height) = img.dimensions();
    Some(eframe::egui::IconData { rgba: img.into_raw(), width, height })
}
