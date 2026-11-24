//! Janela de login. Abre o Google no WebView2, espera voce entrar no YouTube
//! Music e imprime os cookies da sessao (uma linha JSON) no stdout. O app
//! principal chama este exe e nunca ve a sua senha, so os cookies resultantes.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::time::{Duration, Instant};
use tao::dpi::LogicalSize;
use tao::event::{Event, StartCause, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::window::WindowBuilder;
use wry::WebViewBuilder;

const LOGIN_URL: &str =
    "https://accounts.google.com/ServiceLogin?service=youtube&continue=https%3A%2F%2Fmusic.youtube.com%2F";

fn main() {
    let event_loop = EventLoopBuilder::new().build();
    let window = WindowBuilder::new()
        .with_title("Entrar no YouTube Music")
        .with_inner_size(LogicalSize::new(520.0, 740.0))
        .build(&event_loop)
        .expect("janela");

    let webview = WebViewBuilder::new()
        .with_url(LOGIN_URL)
        // Sem UA fixo: o WebView2 usa o do Edge instalado, sempre atual e coerente
        // com o motor real (um UA velho ou diferente do motor faz o Google bloquear).
        // Sessao descartavel: nada do login fica no perfil do WebView2.
        .with_incognito(true)
        .build(&window)
        .expect("webview");

    let mut found = false;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::WaitUntil(Instant::now() + Duration::from_millis(800));
        match event {
            Event::NewEvents(StartCause::ResumeTimeReached { .. }) if !found => {
                let on_music = webview
                    .url()
                    .map(|u| u.starts_with("https://music.youtube.com"))
                    .unwrap_or(false);
                if !on_music {
                    return;
                }
                let Ok(cookies) = webview.cookies_for_url("https://music.youtube.com") else { return };
                let has_session = cookies
                    .iter()
                    .any(|c| matches!(c.name(), "SAPISID" | "__Secure-3PAPISID") && !c.value().is_empty());
                if !has_session {
                    return;
                }
                let header = cookies
                    .iter()
                    .map(|c| format!("{}={}", c.name(), c.value()))
                    .collect::<Vec<_>>()
                    .join("; ");
                println!("{}", serde_json::json!({ "cookie": header }));
                found = true;
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
