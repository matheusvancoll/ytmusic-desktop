//! Teclas de midia (play/pause, proxima, anterior do teclado e do fone) e o
//! painel de midia do Windows, via System Media Transport Controls (SMTC). O
//! Windows entrega as teclas para a sessao de midia ativa, entao nao precisamos
//! de atalhos globais (que brigariam com outros apps).

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MediaAction {
    Play,
    Pause,
    Next,
    Prev,
}

#[cfg(windows)]
pub use imp::MediaControls;

#[cfg(windows)]
mod imp {
    use super::MediaAction;
    use std::sync::mpsc::{Receiver, channel};
    use windows::Foundation::{TypedEventHandler, Uri};
    use windows::Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls, SystemMediaTransportControlsButton,
        SystemMediaTransportControlsButtonPressedEventArgs,
    };
    use windows::Storage::Streams::RandomAccessStreamReference;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::WinRT::ISystemMediaTransportControlsInterop;
    use windows::core::{HSTRING, factory};

    pub struct MediaControls {
        smtc: SystemMediaTransportControls,
        rx: Receiver<MediaAction>,
        /// (video_id, tocando) ja enviados ao Windows, para so atualizar quando muda.
        shown: Option<(String, bool)>,
    }

    impl MediaControls {
        pub fn new(hwnd: isize, ctx: eframe::egui::Context) -> Option<MediaControls> {
            let interop = factory::<SystemMediaTransportControls, ISystemMediaTransportControlsInterop>().ok()?;
            let smtc: SystemMediaTransportControls = unsafe { interop.GetForWindow(HWND(hwnd as *mut _)) }.ok()?;
            smtc.SetIsEnabled(true).ok()?;
            let _ = smtc.SetIsPlayEnabled(true);
            let _ = smtc.SetIsPauseEnabled(true);
            let _ = smtc.SetIsNextEnabled(true);
            let _ = smtc.SetIsPreviousEnabled(true);
            let _ = smtc.SetPlaybackStatus(MediaPlaybackStatus::Closed);
            let (tx, rx) = channel();
            let handler = TypedEventHandler::<SystemMediaTransportControls, SystemMediaTransportControlsButtonPressedEventArgs>::new(
                move |_, args| {
                    if let Some(args) = args.as_ref() {
                        let action = match args.Button()? {
                            SystemMediaTransportControlsButton::Play => Some(MediaAction::Play),
                            SystemMediaTransportControlsButton::Pause => Some(MediaAction::Pause),
                            SystemMediaTransportControlsButton::Next => Some(MediaAction::Next),
                            SystemMediaTransportControlsButton::Previous => Some(MediaAction::Prev),
                            _ => None,
                        };
                        if let Some(a) = action {
                            let _ = tx.send(a);
                            ctx.request_repaint(); // acorda o app mesmo minimizado
                        }
                    }
                    Ok(())
                },
            );
            smtc.ButtonPressed(&handler).ok()?;
            Some(MediaControls { smtc, rx, shown: None })
        }

        /// Chame a cada quadro: atualiza o painel do Windows (so quando algo muda)
        /// e devolve as teclas apertadas.
        pub fn poll(&mut self, current: Option<&crate::ytm::Track>, playing: bool) -> Vec<MediaAction> {
            let key = current.map(|t| (t.video_id.clone(), playing));
            if key != self.shown {
                let id_changed = key.as_ref().map(|k| &k.0) != self.shown.as_ref().map(|k| &k.0);
                self.shown = key;
                let status = match (current, playing) {
                    (None, _) => MediaPlaybackStatus::Closed,
                    (Some(_), true) => MediaPlaybackStatus::Playing,
                    (Some(_), false) => MediaPlaybackStatus::Paused,
                };
                let _ = self.smtc.SetPlaybackStatus(status);
                if let (true, Some(t)) = (id_changed, current) {
                    let _ = self.update_display(t);
                }
            }
            self.rx.try_iter().collect()
        }

        fn update_display(&self, t: &crate::ytm::Track) -> windows::core::Result<()> {
            let du = self.smtc.DisplayUpdater()?;
            du.SetType(MediaPlaybackType::Music)?;
            let props = du.MusicProperties()?;
            props.SetTitle(&HSTRING::from(t.title.as_str()))?;
            props.SetArtist(&HSTRING::from(t.artists.as_str()))?;
            props.SetAlbumTitle(&HSTRING::from(t.album.as_str()))?;
            if !t.thumb.is_empty() {
                // O Windows baixa a capa sozinho, sob demanda (nada fica na memoria do app).
                let url = crate::ytm::thumb_url(&t.thumb, 300);
                if let Ok(uri) = Uri::CreateUri(&HSTRING::from(url)) {
                    if let Ok(r) = RandomAccessStreamReference::CreateFromUri(&uri) {
                        let _ = du.SetThumbnail(&r);
                    }
                }
            }
            du.Update()
        }
    }
}
