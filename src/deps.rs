//! Componentes externos que o app usa para tocar: o yt-dlp (baixa o audio) e um
//! runtime JavaScript (o yt-dlp precisa de um para o YouTube). Quem usa o instalador
//! ja recebe os dois pelo winget; quem roda so o `.exe` pode baixa-los por aqui,
//! sempre a pedido (botao), dos Releases oficiais no GitHub e com o SHA-256 conferido.
//!
//! Os arquivos ficam em `%LOCALAPPDATA%\ytmusic` (`yt-dlp.exe` e `deno.exe`), onde
//! `player::find_ytdlp` e `player::js_runtime_arg` ja procuram.

use crate::{player, updater, util};
use std::path::Path;

const YTDLP_REPO: &str = "yt-dlp/yt-dlp";
const YTDLP_ASSET: &str = "yt-dlp.exe";
const DENO_REPO: &str = "denoland/deno";
const DENO_ASSET: &str = "deno-x86_64-pc-windows-msvc.zip";

/// Componente sendo baixado agora (para a barra de progresso).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Component {
    YtDlp,
    Deno,
}

impl Component {
    pub fn label(self) -> &'static str {
        match self {
            Component::YtDlp => "yt-dlp",
            Component::Deno => "Deno",
        }
    }
}

/// O que falta para tocar musicas.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Missing {
    pub ytdlp: bool,
    pub js_runtime: bool,
}

impl Missing {
    pub fn any(self) -> bool {
        self.ytdlp || self.js_runtime
    }
}

pub fn missing() -> Missing {
    Missing { ytdlp: player::find_ytdlp().is_none(), js_runtime: player::js_runtime_arg().is_none() }
}

/// Baixa e instala o que estiver faltando. `progress(componente, baixado, total)`.
pub fn install_missing(mut progress: impl FnMut(Component, u64, u64)) -> Result<(), String> {
    let need = missing();
    let dir = util::data_dir();
    if need.ytdlp {
        let asset = updater::latest_asset(YTDLP_REPO, YTDLP_ASSET)?;
        updater::download_verified(&asset.url, &asset.sha256, asset.size, &dir.join("yt-dlp.exe"), |d, t| {
            progress(Component::YtDlp, d, t)
        })?;
    }
    if need.js_runtime {
        let asset = updater::latest_asset(DENO_REPO, DENO_ASSET)?;
        let zip = dir.join("deno-download.zip");
        updater::download_verified(&asset.url, &asset.sha256, asset.size, &zip, |d, t| progress(Component::Deno, d, t))?;
        let result = extract_zip(&zip, &dir);
        let _ = std::fs::remove_file(&zip);
        result?;
        if !dir.join("deno.exe").is_file() {
            return Err("O pacote do Deno não trouxe o deno.exe.".into());
        }
    }
    Ok(())
}

/// Extrai um .zip com o `tar.exe` que vem no Windows 10 (1803+) e 11.
fn extract_zip(zip: &Path, dest: &Path) -> Result<(), String> {
    let tar = std::env::var_os("SystemRoot")
        .map(|r| Path::new(&r).join("System32").join("tar.exe"))
        .filter(|p| p.is_file())
        .ok_or("O Windows não tem o tar.exe para extrair o Deno.")?;
    let mut cmd = std::process::Command::new(tar);
    cmd.arg("-xf").arg(zip).arg("-C").arg(dest);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let status = cmd.status().map_err(|e| format!("Não foi possível extrair o Deno: {e}"))?;
    if status.success() { Ok(()) } else { Err("Não foi possível extrair o Deno.".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Baixa de verdade (precisa de internet) numa pasta temporaria, sem tocar no app.
    /// Rode com `cargo test live_deps -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_deps_assets_are_published_with_sha256() {
        for (repo, name) in [(YTDLP_REPO, YTDLP_ASSET), (DENO_REPO, DENO_ASSET)] {
            let a = updater::latest_asset(repo, name).expect(name);
            println!("{name}: {} bytes, sha256 {}", a.size, a.sha256);
            assert!(a.size > 1_000_000);
        }
    }

    /// Download real e conferido dos dois componentes numa pasta temporaria, e cada um
    /// responde `--version`. Rode com `cargo test live_deps -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn live_deps_download_extract_and_run() {
        let dir = std::env::temp_dir().join(format!("ytmusic-live-deps-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = updater::latest_asset(YTDLP_REPO, YTDLP_ASSET).unwrap();
        updater::download_verified(&a.url, &a.sha256, a.size, &dir.join("yt-dlp.exe"), |_, _| {}).unwrap();
        let a = updater::latest_asset(DENO_REPO, DENO_ASSET).unwrap();
        let zip = dir.join("deno.zip");
        updater::download_verified(&a.url, &a.sha256, a.size, &zip, |_, _| {}).unwrap();
        extract_zip(&zip, &dir).unwrap();
        for exe in ["yt-dlp.exe", "deno.exe"] {
            let out = std::process::Command::new(dir.join(exe)).arg("--version").output().unwrap();
            println!("{exe}: {}", String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or(""));
            assert!(out.status.success(), "{exe}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extract_zip_works_with_windows_tar() {
        // Monta um zip minimo com o proprio tar do Windows e extrai de volta.
        let base = std::env::temp_dir().join(format!("ytmusic-deps-test-{}", std::process::id()));
        let (src, out) = (base.join("src"), base.join("out"));
        std::fs::create_dir_all(&src).unwrap();
        std::fs::create_dir_all(&out).unwrap();
        std::fs::write(src.join("deno.exe"), b"fake").unwrap();
        let zip = base.join("t.zip");
        let tar = Path::new(&std::env::var_os("SystemRoot").unwrap()).join("System32").join("tar.exe");
        let ok = std::process::Command::new(tar).arg("-a").arg("-cf").arg(&zip).arg("-C").arg(&src).arg("deno.exe").status().unwrap();
        assert!(ok.success());
        extract_zip(&zip, &out).unwrap();
        assert_eq!(std::fs::read(out.join("deno.exe")).unwrap(), b"fake");
        let _ = std::fs::remove_dir_all(&base);
    }
}
