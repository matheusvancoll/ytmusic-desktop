//! Reproducao de audio: `yt-dlp` baixa o audio da faixa para o cache e o rodio
//! (symphonia, em Rust puro) decodifica e toca. Um thread dono do dispositivo
//! de audio recebe comandos; a UI so le o estado compartilhado.

use crate::util;
use crate::ytm::Track;
use rodio::{Decoder, DeviceSinkBuilder, Source};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Idle,
    Loading,
    Playing,
    Paused,
    Error(String),
}

#[derive(Clone, Debug)]
pub struct PlayerState {
    pub status: Status,
    pub current: Option<Track>,
    pub position: Duration,
    pub duration: Duration,
    pub volume: f32,
    /// A faixa acabou (ou falhou) e a UI deve avancar a fila. A UI zera.
    pub finished: bool,
    /// Em `Loading`: a faixa ainda nao esta no cache e o yt-dlp esta baixando.
    pub downloading: bool,
    /// O automix ja comecou a proxima faixa sozinho: a UI so avanca a fila, sem
    /// mandar tocar de novo. A UI zera.
    pub advanced: bool,
}

enum Cmd {
    Play(Track),
    Toggle,
    Seek(Duration),
    Volume(f32),
    /// Duracao da mescla em segundos; 0 desliga o automix.
    Crossfade(u32),
    /// Faixa que a fila tocara depois desta (a UI mantem atualizada).
    SetNext(Option<Track>),
    Ready { token: u64, path: PathBuf },
    Failed { token: u64, message: String },
}

pub struct Player {
    tx: Sender<Cmd>,
    pub state: Arc<Mutex<PlayerState>>,
    ctx: eframe::egui::Context,
}

impl Player {
    pub fn new(ctx: eframe::egui::Context) -> Player {
        let state = Arc::new(Mutex::new(PlayerState {
            status: Status::Idle,
            current: None,
            position: Duration::ZERO,
            duration: Duration::ZERO,
            volume: load_volume(),
            finished: false,
            downloading: false,
            advanced: false,
        }));
        let (tx, rx) = channel();
        let (worker_state, worker_tx, worker_ctx) = (state.clone(), tx.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("audio".into())
            .spawn(move || worker(rx, worker_tx, worker_state, worker_ctx))
            .expect("thread de audio");
        std::thread::spawn(trim_cache);
        Player { tx, state, ctx }
    }

    pub fn snapshot(&self) -> PlayerState {
        self.state.lock().unwrap().clone()
    }

    pub fn play(&self, track: Track) {
        let _ = self.tx.send(Cmd::Play(track));
    }

    pub fn toggle(&self) {
        let _ = self.tx.send(Cmd::Toggle);
    }

    pub fn seek(&self, to: Duration) {
        let _ = self.tx.send(Cmd::Seek(to));
    }

    pub fn set_volume(&self, v: f32) {
        let _ = self.tx.send(Cmd::Volume(v.clamp(0.0, 1.0)));
    }

    /// Liga o automix com a mescla de `secs` segundos; 0 desliga.
    pub fn set_crossfade(&self, secs: u32) {
        let _ = self.tx.send(Cmd::Crossfade(secs));
    }

    /// Informa qual faixa vem depois desta (para o automix poder comecar a mescla).
    pub fn set_next(&self, track: Option<Track>) {
        let _ = self.tx.send(Cmd::SetNext(track));
    }

    /// Baixa a proxima faixa em segundo plano para a troca ser instantanea.
    pub fn prefetch(&self, video_id: &str) {
        let id = video_id.to_string();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = ensure_downloaded(&id);
            ctx.request_repaint();
        });
    }
}

/// Mescla em andamento: a faixa que esta acabando (`out`) sai enquanto a que entrou
/// (o `player` do worker) sobe. O dispositivo antigo fica aberto ate o fim da mescla.
struct Fade {
    out: rodio::Player,
    _old_sink: Option<rodio::MixerDeviceSink>,
    len: Duration,
}

/// Ganhos (entra, sai) em `progress` de 0 a 1. Potencia constante: entra^2 + sai^2 = 1,
/// entao o volume percebido nao afunda nem estoura no meio da mescla.
fn fade_gains(progress: f32) -> (f32, f32) {
    let angle = progress.clamp(0.0, 1.0) * std::f32::consts::FRAC_PI_2;
    (angle.sin(), angle.cos())
}

/// Faixa a mesclar agora, se for a hora: automix ligado, proxima conhecida e ja
/// baixada, e o fim da atual dentro da janela. Devolve (arquivo, faixa, duracao da mescla).
fn crossfade_due(
    crossfade_secs: u32,
    next: &Option<Track>,
    player: &rodio::Player,
    state: &Arc<Mutex<PlayerState>>,
) -> Option<(PathBuf, Track, Duration)> {
    let upcoming = next.as_ref().filter(|_| crossfade_secs > 0)?;
    let (status, duration) = {
        let s = state.lock().unwrap();
        (s.status.clone(), s.duration)
    };
    let window = Duration::from_secs(crossfade_secs as u64);
    let remaining = duration.saturating_sub(player.get_pos());
    // Faixas curtas demais (menos de 3 janelas) nao mesclam; ao tocar o fim isso evita
    // uma mescla maior que a propria musica.
    let due = status == Status::Playing
        && !player.is_paused()
        && duration >= window * 3
        && remaining >= Duration::from_secs(1)
        && remaining <= window;
    if !due {
        return None;
    }
    let path = cached_file(&upcoming.video_id)?; // ainda nao baixou: troca normal
    Some((path, upcoming.clone(), window.min(remaining)))
}

fn worker(rx: Receiver<Cmd>, tx: Sender<Cmd>, state: Arc<Mutex<PlayerState>>, ctx: eframe::egui::Context) {
    let mut sink: Option<rodio::MixerDeviceSink> = None;
    let mut player: Option<rodio::Player> = None;
    let mut token: u64 = 0;
    let mut retried: Vec<String> = Vec::new();
    let mut fade: Option<Fade> = None;
    let mut crossfade_secs: u32 = 0;
    let mut next: Option<Track> = None;

    loop {
        // Durante a mescla o volume e atualizado com mais frequencia (degraus menores).
        let tick = Duration::from_millis(if fade.is_some() { 40 } else { 150 });
        match rx.recv_timeout(tick) {
            Ok(Cmd::Play(track)) => {
                token += 1;
                if let Some(f) = fade.take() {
                    f.out.stop();
                }
                if let Some(p) = player.take() {
                    p.stop();
                }
                {
                    let mut s = state.lock().unwrap();
                    s.status = Status::Loading;
                    s.position = Duration::ZERO;
                    s.duration = Duration::from_secs(track.duration_secs as u64);
                    s.current = Some(track.clone());
                    s.finished = false;
                    s.downloading = cached_file(&track.video_id).is_none();
                }
                let (my_token, tx2, ctx2) = (token, tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let cmd = match ensure_downloaded(&track.video_id) {
                        Ok(path) => Cmd::Ready { token: my_token, path },
                        Err(message) => Cmd::Failed { token: my_token, message },
                    };
                    let _ = tx2.send(cmd);
                    ctx2.request_repaint();
                });
            }
            Ok(Cmd::Ready { token: t, path }) if t == token => match start(&path, &mut sink, &state, 1.0) {
                Ok((p, total)) => {
                    player = Some(p);
                    let mut s = state.lock().unwrap();
                    if let Some(t) = total {
                        s.duration = t;
                    }
                    s.status = Status::Playing;
                }
                Err(message) => {
                    // Arquivo do cache corrompido (ex.: download antigo interrompido):
                    // apaga e baixa de novo, uma vez so.
                    let id = state.lock().unwrap().current.as_ref().map(|t| t.video_id.clone());
                    let _ = std::fs::remove_file(&path);
                    match id {
                        Some(id) if !retried.contains(&id) => {
                            retried.push(id.clone());
                            let (my_token, tx2, ctx2) = (token, tx.clone(), ctx.clone());
                            std::thread::spawn(move || {
                                let cmd = match ensure_downloaded(&id) {
                                    Ok(path) => Cmd::Ready { token: my_token, path },
                                    Err(message) => Cmd::Failed { token: my_token, message },
                                };
                                let _ = tx2.send(cmd);
                                ctx2.request_repaint();
                            });
                        }
                        _ => fail(&state, message),
                    }
                }
            },
            Ok(Cmd::Failed { token: t, message }) if t == token => fail(&state, message),
            Ok(Cmd::Ready { .. }) | Ok(Cmd::Failed { .. }) => {} // resposta de uma faixa antiga
            Ok(Cmd::Toggle) => {
                if let Some(p) = &player {
                    let mut s = state.lock().unwrap();
                    if p.is_paused() {
                        p.play();
                        if let Some(f) = &fade {
                            f.out.play();
                        }
                        s.status = Status::Playing;
                    } else {
                        p.pause();
                        if let Some(f) = &fade {
                            f.out.pause();
                        }
                        s.status = Status::Paused;
                    }
                }
            }
            Ok(Cmd::Seek(to)) => {
                if let Some(p) = &player {
                    // Mexer na barra cancela a mescla: a faixa nova assume o volume cheio.
                    if let Some(f) = fade.take() {
                        f.out.stop();
                        let v = state.lock().unwrap().volume;
                        p.set_volume(v * v);
                    }
                    let _ = p.try_seek(to);
                }
            }
            Ok(Cmd::Volume(v)) => {
                state.lock().unwrap().volume = v;
                save_volume(v);
                if let Some(p) = &player {
                    p.set_volume(v * v);
                }
            }
            Ok(Cmd::Crossfade(secs)) => crossfade_secs = secs,
            Ok(Cmd::SetNext(track)) => next = track,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        // Mescla em andamento: segue as curvas de volume e encerra quando completar.
        if let (Some(f), Some(p)) = (&fade, &player) {
            let base = {
                let v = state.lock().unwrap().volume;
                v * v
            };
            let progress = p.get_pos().as_secs_f32() / f.len.as_secs_f32().max(0.001);
            if progress >= 1.0 || f.out.empty() {
                p.set_volume(base);
                if let Some(f) = fade.take() {
                    f.out.stop();
                }
            } else {
                let (gain_in, gain_out) = fade_gains(progress);
                p.set_volume(base * gain_in);
                f.out.set_volume(base * gain_out);
            }
        }

        // Fim da faixa perto e automix ligado: comeca a proxima por cima, em outro canal.
        if fade.is_none() {
            let due = player.as_ref().and_then(|p| crossfade_due(crossfade_secs, &next, p, &state));
            if let Some((path, upcoming, len)) = due {
                let old_sink = sink.take();
                match start(&path, &mut sink, &state, 0.0) {
                    Ok((incoming, total)) => {
                        if let Some(out) = player.replace(incoming) {
                            fade = Some(Fade { out, _old_sink: old_sink, len });
                        }
                        next = None; // a UI manda a seguinte depois de avancar a fila
                        let mut s = state.lock().unwrap();
                        s.duration = total.unwrap_or(Duration::from_secs(upcoming.duration_secs as u64));
                        s.current = Some(upcoming);
                        s.position = Duration::ZERO;
                        s.status = Status::Playing;
                        s.finished = false;
                        s.advanced = true;
                        drop(s);
                        ctx.request_repaint();
                    }
                    // Nao deu para abrir a proxima: segue a faixa atual e troca normalmente no fim.
                    Err(_) => {
                        sink = old_sink;
                        next = None;
                    }
                }
            }
        }

        if let Some(p) = &player {
            let mut s = state.lock().unwrap();
            s.position = p.get_pos();
            if p.empty() && s.status == Status::Playing {
                s.status = Status::Idle;
                s.finished = true;
                drop(s);
                player = None;
                ctx.request_repaint();
            }
        }
    }
}

fn fail(state: &Arc<Mutex<PlayerState>>, message: String) {
    let mut s = state.lock().unwrap();
    s.status = Status::Error(message);
    s.finished = true;
}

/// Abre o dispositivo e comeca a tocar `path` com ganho inicial `gain` (0 = mudo,
/// para a mescla subir do zero sem estalo). Devolve o player e a duracao real, se conhecida.
fn start(
    path: &Path,
    sink: &mut Option<rodio::MixerDeviceSink>,
    state: &Arc<Mutex<PlayerState>>,
    gain: f32,
) -> Result<(rodio::Player, Option<Duration>), String> {
    // Reabre o dispositivo a cada faixa: sobrevive a troca de fone/saida.
    let mut device = DeviceSinkBuilder::open_default_sink().map_err(|e| format!("audio: {e}"))?;
    device.log_on_drop(false);
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let decoder = Decoder::try_from(file).map_err(|e| format!("formato nao suportado: {e}"))?;
    let total = decoder.total_duration();
    let player = rodio::Player::connect_new(device.mixer());
    let volume = state.lock().unwrap().volume;
    player.set_volume(volume * volume * gain);
    player.append(decoder);
    *sink = Some(device);
    Ok((player, total))
}

// ------------------------------------------------------------------ yt-dlp

fn locks() -> &'static Mutex<HashMap<String, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(Default::default)
}

fn cached_file(id: &str) -> Option<PathBuf> {
    std::fs::read_dir(util::cache_dir())
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| is_final_audio(p, id))
}

fn which(name: &str) -> Option<PathBuf> {
    let exe = format!("{name}.exe");
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|d| d.join(&exe))
            .find(|p| p.is_file())
    })
}

/// Procura `exe` na pasta de pacotes do winget (`<package_prefix>_*`). O PATH
/// do processo pode estar desatualizado logo apos instalar, entao olhamos direto la.
fn winget_package_exe(package_prefix: &str, exe: &str) -> Option<PathBuf> {
    let packages = dirs::data_local_dir()?.join("Microsoft").join("WinGet").join("Packages");
    std::fs::read_dir(packages)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(package_prefix))
        .map(|e| e.path().join(exe))
        .find(|p| p.is_file())
}

pub fn find_ytdlp() -> Option<PathBuf> {
    let mut candidates = vec![util::data_dir().join("yt-dlp.exe")];
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("yt-dlp.exe"));
        }
    }
    candidates.extend(winget_package_exe("yt-dlp.yt-dlp_", "yt-dlp.exe"));
    if let Some(found) = candidates.into_iter().find(|p| p.is_file()) {
        return Some(found);
    }
    which("yt-dlp")
}

/// Argumento `--js-runtimes` para o yt-dlp: Deno (o baixado pelo app, o do PATH ou o
/// do winget) ou Node.
pub(crate) fn js_runtime_arg() -> Option<String> {
    let own = Some(util::data_dir().join("deno.exe")).filter(|p| p.is_file());
    if let Some(p) = own.or_else(|| which("deno")).or_else(|| winget_package_exe("DenoLand.Deno_", "deno.exe")) {
        return Some(format!("deno:{}", p.display()));
    }
    which("node").map(|p| format!("node:{}", p.display()))
}

fn ensure_downloaded(id: &str) -> Result<PathBuf, String> {
    if let Some(p) = cached_file(id) {
        return Ok(p);
    }
    let lock = locks().lock().unwrap().entry(id.to_string()).or_default().clone();
    let _guard = lock.lock().unwrap();
    if let Some(p) = cached_file(id) {
        return Ok(p); // outro thread terminou enquanto esperavamos
    }

    let ytdlp = find_ytdlp().ok_or_else(|| crate::tr!("player.ytdlp_missing").to_string())?;
    // Baixa numa pasta temporaria e so move para o cache quando o yt-dlp termina
    // (inclusive o conserto do container pelo ffmpeg). Antes o arquivo parcial ja
    // aparecia no cache com o nome final, e tocar a faixa durante o download dava
    // "formato nao suportado".
    let staging = staging_dir();
    clear_staging(id);
    let mut cmd = Command::new(ytdlp);
    cmd.args(["-f", "bestaudio[ext=m4a]/bestaudio[acodec^=mp4a]/140"])
        .args(["--no-playlist", "--no-warnings", "--no-progress", "-q"])
        .arg("-o")
        .arg(staging.join("%(id)s.%(ext)s"));
    // Versoes recentes do yt-dlp precisam de um runtime JS para o YouTube.
    if let Some(rt) = js_runtime_arg() {
        cmd.args(["--js-runtimes", &rt]);
    }
    cmd.arg(format!("https://music.youtube.com/watch?v={id}"));
    hide_console(&mut cmd);
    let out = cmd.output().map_err(|e| format!("falha ao executar yt-dlp: {e}"))?;
    if !out.status.success() {
        clear_staging(id);
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("erro desconhecido");
        return Err(format!("yt-dlp: {}", last.trim()));
    }
    let done = std::fs::read_dir(&staging)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .find(|p| is_final_audio(p, id))
        .ok_or("o yt-dlp nao gerou arquivo de audio")?;
    let dest = util::cache_dir().join(done.file_name().unwrap_or_default());
    std::fs::rename(&done, &dest).map_err(|e| format!("cache: {e}"))?;
    clear_staging(id);
    Ok(dest)
}

/// `<cache>/tmp`: downloads em andamento.
fn staging_dir() -> PathBuf {
    let dir = util::cache_dir().join("tmp");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// `<id>.<ext>` final (nao `.part`, `.ytdl`, `.temp.m4a` etc.).
fn is_final_audio(p: &Path, id: &str) -> bool {
    p.is_file()
        && p.file_stem().and_then(|s| s.to_str()) == Some(id)
        && p.extension().is_some_and(|e| e != "part" && e != "ytdl")
}

/// Apaga restos de um download anterior desta faixa.
fn clear_staging(id: &str) {
    let Ok(rd) = std::fs::read_dir(staging_dir()) else { return };
    for e in rd.flatten() {
        if e.file_name().to_string_lossy().starts_with(&format!("{id}.")) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(windows)]
fn hide_console(cmd: &mut Command) {
    use std::os::windows::process::CommandExt;
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}

#[cfg(not(windows))]
fn hide_console(_: &mut Command) {}

/// Mantem so as 80 faixas mais recentes no cache (~4 MB cada).
fn trim_cache() {
    // Downloads interrompidos de sessoes anteriores.
    let _ = std::fs::remove_dir_all(staging_dir());
    let Ok(rd) = std::fs::read_dir(util::cache_dir()) else { return };
    let mut files: Vec<_> = rd
        .flatten()
        .filter(|e| e.path().is_file())
        .filter(|e| {
            // Restos do formato antigo (arquivo parcial gravado direto no cache).
            let name = e.file_name().to_string_lossy().to_string();
            let junk = name.contains(".temp.") || name.ends_with(".part") || name.ends_with(".ytdl");
            if junk {
                let _ = std::fs::remove_file(e.path());
            }
            !junk
        })
        .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in files.into_iter().skip(80) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Baixa uma faixa de verdade e confere que o rodio decodifica e busca.
    /// Rode com `cargo test download_and_decode -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn download_and_decode() {
        let path = ensure_downloaded("Jb6gcoR266U").expect("download");
        println!("arquivo: {} ({} bytes)", path.display(), std::fs::metadata(&path).unwrap().len());
        let file = std::fs::File::open(&path).unwrap();
        let dec = Decoder::try_from(file).expect("decoder");
        println!("duracao: {:?}, canais: {:?}, taxa: {:?}", dec.total_duration(), dec.channels(), dec.sample_rate());
        let secs = dec.take_duration(Duration::from_secs(2)).count();
        assert!(secs > 0, "nenhuma amostra decodificada");
        println!("amostras em 2s: {secs}");
    }

    /// Automix de ponta a ponta com o worker real e o dispositivo de audio: toca A,
    /// pula para perto do fim e confere que B entra sozinho (flag `advanced`) e que
    /// a mescla termina com B tocando normalmente. Usa volume baixo sem gravar o volume do usuario.
    /// Precisa das duas faixas no cache. Rode com
    /// `cargo test automix_crossfades -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn automix_crossfades() {
        fn track(id: &str) -> Track {
            let path = cached_file(id).unwrap_or_else(|| panic!("{id} nao esta no cache"));
            let total = Decoder::try_from(std::fs::File::open(path).unwrap()).unwrap().total_duration().unwrap();
            Track { video_id: id.into(), title: id.into(), duration_secs: total.as_secs() as u32, ..Default::default() }
        }
        let (a, b) = (track("Jb6gcoR266U"), track("dQw4w9WgXcQ"));
        let a_len = Duration::from_secs(a.duration_secs as u64);

        let player = Player::new(eframe::egui::Context::default());
        player.state.lock().unwrap().volume = 0.05; // direto no estado: set_volume() gravaria no arquivo do usuario
        player.play(a.clone());
        let wait = |what: &str, secs: u64, cond: &dyn Fn(&PlayerState) -> bool| {
            let start = std::time::Instant::now();
            while !cond(&player.snapshot()) {
                assert!(start.elapsed() < Duration::from_secs(secs), "tempo esgotado: {what}");
                std::thread::sleep(Duration::from_millis(50));
            }
            start.elapsed()
        };
        wait("A tocando", 20, &|s| s.status == Status::Playing);

        player.set_crossfade(3);
        player.set_next(Some(b.clone()));
        player.seek(a_len - Duration::from_secs(7));
        std::thread::sleep(Duration::from_secs(2));
        let s = player.snapshot();
        assert!(!s.advanced && s.current.as_ref().unwrap().video_id == a.video_id, "mesclou cedo demais");

        let took = wait("B entrar sozinho", 15, &|s| s.advanced);
        let s = player.snapshot();
        println!("B entrou apos {took:?}; posicao de B {:?}, duracao {:?}", s.position, s.duration);
        assert_eq!(s.current.as_ref().unwrap().video_id, b.video_id);
        assert!(s.position < Duration::from_secs(2), "B deveria comecar do zero");
        assert!(!s.finished, "nao pode contar como fim de faixa");

        // Passada a mescla (3s) e o fim de A, B segue tocando e avancando.
        std::thread::sleep(Duration::from_secs(5));
        let s = player.snapshot();
        println!("depois da mescla: {:?} em {:?}", s.status, s.position);
        assert_eq!(s.status, Status::Playing);
        assert!(!s.finished && s.position > Duration::from_secs(4), "B parou ou nao avancou");
        assert_eq!(s.current.as_ref().unwrap().video_id, b.video_id);

        // Desligado (0), nao mescla: so toca A ate o fim e marca `finished`.
        player.state.lock().unwrap().advanced = false; // a UI zera a flag; aqui fazemos o mesmo
        player.set_crossfade(0);
        player.set_next(Some(a.clone()));
        player.play(a.clone());
        wait("A tocando de novo", 20, &|s| s.status == Status::Playing && s.current.as_ref().is_some_and(|t| t.video_id == a.video_id));
        player.seek(a_len - Duration::from_secs(4));
        std::thread::sleep(Duration::from_secs(1));
        assert!(!player.snapshot().advanced, "mesclou com o automix desligado");
        wait("fim de A", 15, &|s| s.finished);
        assert!(!player.snapshot().advanced);
    }

    /// Toca 3s no dispositivo de audio padrao e confere que a posicao avanca e
    /// que o seek funciona. Precisa de uma saida de audio.
    #[test]
    #[ignore]
    fn plays_and_seeks() {
        let path = ensure_downloaded("Jb6gcoR266U").expect("download");
        let mut device = DeviceSinkBuilder::open_default_sink().expect("saida de audio");
        device.log_on_drop(false);
        let player = rodio::Player::connect_new(device.mixer());
        player.set_volume(0.05);
        player.append(Decoder::try_from(std::fs::File::open(path).unwrap()).unwrap());
        std::thread::sleep(Duration::from_secs(3));
        let p1 = player.get_pos();
        println!("pos apos 3s: {p1:?}");
        assert!(p1 > Duration::from_secs(2), "posicao nao avancou");
        player.try_seek(Duration::from_secs(120)).expect("seek");
        std::thread::sleep(Duration::from_millis(500));
        let p2 = player.get_pos();
        println!("pos apos seek: {p2:?}");
        assert!(p2 > Duration::from_secs(119) && p2 < Duration::from_secs(123), "seek falhou: {p2:?}");
        assert!(!player.empty());
    }
}

fn volume_file() -> PathBuf {
    util::data_dir().join("volume")
}

fn load_volume() -> f32 {
    std::fs::read_to_string(volume_file())
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
        .map(|v| v.clamp(0.0, 1.0))
        .unwrap_or(0.7)
}

fn save_volume(v: f32) {
    let _ = std::fs::write(volume_file(), v.to_string());
}

// ------------------------------------------------------------------ automix

/// Limites da duracao da mescla, em segundos.
pub const AUTOMIX_MIN_SECS: u32 = 2;
pub const AUTOMIX_MAX_SECS: u32 = 12;
const AUTOMIX_DEFAULT_SECS: u32 = 6;

fn automix_file() -> PathBuf {
    util::data_dir().join("automix")
}

/// `(ligado, segundos)`. Sem arquivo (ou arquivo ilegivel) = desligado.
fn parse_automix(text: &str) -> (bool, u32) {
    let mut parts = text.split_whitespace();
    let enabled = parts.next() == Some("on");
    let secs = parts
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(AUTOMIX_DEFAULT_SECS)
        .clamp(AUTOMIX_MIN_SECS, AUTOMIX_MAX_SECS);
    (enabled, secs)
}

pub fn load_automix() -> (bool, u32) {
    parse_automix(&std::fs::read_to_string(automix_file()).unwrap_or_default())
}

pub fn save_automix(enabled: bool, secs: u32) {
    let _ = std::fs::write(automix_file(), format!("{} {secs}", if enabled { "on" } else { "off" }));
}

#[cfg(test)]
mod automix_tests {
    use super::*;

    #[test]
    fn fade_keeps_constant_power_and_ends_at_the_right_levels() {
        assert_eq!(fade_gains(0.0), (0.0, 1.0));
        let (end_in, end_out) = fade_gains(1.0);
        assert!((end_in - 1.0).abs() < 1e-6 && end_out.abs() < 1e-6);
        for i in 0..=20 {
            let (a, b) = fade_gains(i as f32 / 20.0);
            assert!((a * a + b * b - 1.0).abs() < 1e-5, "potencia nao constante em {i}/20");
        }
        // Fora do intervalo nao passa de 0..1.
        assert_eq!(fade_gains(-3.0), fade_gains(0.0));
        assert_eq!(fade_gains(9.0), fade_gains(1.0));
    }

    #[test]
    fn automix_is_off_by_default_and_settings_are_clamped() {
        assert_eq!(parse_automix(""), (false, AUTOMIX_DEFAULT_SECS));
        assert_eq!(parse_automix("lixo"), (false, AUTOMIX_DEFAULT_SECS));
        assert_eq!(parse_automix("on 8"), (true, 8));
        assert_eq!(parse_automix("off 4"), (false, 4));
        assert_eq!(parse_automix("on 99"), (true, AUTOMIX_MAX_SECS));
        assert_eq!(parse_automix("on 0"), (true, AUTOMIX_MIN_SECS));
        assert_eq!(parse_automix("on"), (true, AUTOMIX_DEFAULT_SECS));
    }
}
