//! Atualizacao do app pelos Releases do GitHub.
//!
//! Fluxo: consulta o ultimo Release do repositorio (`repository` do Cargo.toml),
//! compara com a versao em execucao, baixa o instalador, confere o SHA-256 e o
//! executa em modo silencioso. O app fecha para o instalador poder substituir os
//! arquivos, e o instalador reabre o app no final.
//!
//! Privacidade: so conversa com api.github.com e github.com, sem enviar nada alem
//! do pedido HTTP comum (User-Agent generico, sem identificador de usuario).

use crate::util;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Versao em execucao (a do Cargo.toml).
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
const API_BASE: &str = "https://api.github.com";
/// O instalador so e baixado destes enderecos.
const TRUSTED_DOWNLOADS: &[&str] = &["https://github.com/"];
const UA: &str = "ytmusic-updater";
const MAX_DOWNLOAD: u64 = 200 * 1024 * 1024;
/// Intervalo entre as verificacoes automaticas.
pub const CHECK_EVERY_SECS: u64 = 24 * 60 * 60;

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    /// Endereco do instalador (`YTMusic-Setup-<versao>.exe`).
    pub url: String,
    /// SHA-256 esperado do instalador, em hexadecimal minusculo.
    pub sha256: String,
    pub size: u64,
}

/// `dono/repositorio` lido do campo `repository` do Cargo.toml.
pub fn repo_slug() -> Option<String> {
    let rest = env!("CARGO_PKG_REPOSITORY").strip_prefix("https://github.com/")?;
    let slug = rest.trim_end_matches('/').trim_end_matches(".git");
    (slug.matches('/').count() == 1 && !slug.starts_with('/') && !slug.ends_with('/')).then(|| slug.to_string())
}

pub fn parse_version(s: &str) -> Option<[u64; 3]> {
    let s = s.trim().trim_start_matches(['v', 'V']);
    let core = s.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some([major, minor, patch])
}

pub fn is_newer(remote: &str, current: &str) -> bool {
    match (parse_version(remote), parse_version(current)) {
        (Some(r), Some(c)) => r > c,
        _ => false,
    }
}

/// Primeiro trecho de exatamente 64 caracteres hexadecimais depois de "SHA-256" no texto.
fn extract_sha256(text: &str) -> Option<String> {
    let lower = text.to_ascii_lowercase();
    let start = lower.find("sha-256").or_else(|| lower.find("sha256"))?;
    let rest = &lower[start..];
    let bytes = rest.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_hexdigit() {
            let end = bytes[i..].iter().position(|b| !b.is_ascii_hexdigit()).map_or(bytes.len(), |p| i + p);
            if end - i == 64 {
                return Some(rest[i..end].to_string());
            }
            i = end;
        } else {
            i += 1;
        }
    }
    None
}

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

fn agent(timeout_secs: u64) -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(timeout_secs))).build().into()
}

fn net_error(e: &ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(403) | ureq::Error::StatusCode(429) => {
            "O GitHub limitou as consultas por agora. Tente de novo mais tarde.".into()
        }
        ureq::Error::StatusCode(code) => format!("O GitHub respondeu com erro {code}."),
        other => format!("Sem conexão com o GitHub ({other})."),
    }
}

/// Le o JSON de um Release e devolve a atualizacao, se for mais nova que `current`.
fn parse_release(v: &Value, current: &str, trusted: &[&str]) -> Result<Option<Release>, String> {
    let tag = v["tag_name"].as_str().ok_or("Release sem versão.")?;
    let version = tag.trim_start_matches(['v', 'V']).to_string();
    if !is_newer(&version, current) {
        return Ok(None);
    }
    let wanted = format!("YTMusic-Setup-{version}.exe");
    let asset = v["assets"]
        .as_array()
        .and_then(|a| a.iter().find(|a| a["name"].as_str() == Some(wanted.as_str())))
        .ok_or_else(|| format!("O Release {tag} não tem o instalador {wanted}."))?;
    let url = asset["browser_download_url"].as_str().ok_or("Instalador sem endereço de download.")?.to_string();
    if !trusted.iter().any(|p| url.starts_with(p)) {
        return Err("Endereço de download não confiável. Atualização cancelada.".into());
    }
    let notes = v["body"].as_str().unwrap_or("").to_string();

    // O hash vem do campo `digest` do arquivo e/ou das notas do Release. Se os dois existirem, precisam bater.
    let from_digest = asset["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(str::to_ascii_lowercase).filter(|h| is_sha256_hex(h));
    let from_notes = extract_sha256(&notes);
    let sha256 = match (from_digest, from_notes) {
        (Some(a), Some(b)) if a != b => return Err("O hash do instalador diverge entre o arquivo e as notas do Release. Atualização cancelada.".into()),
        (Some(h), _) | (None, Some(h)) => h,
        (None, None) => return Err("O Release não publica o SHA-256 do instalador; não dá para verificar o download.".into()),
    };
    Ok(Some(Release { version, notes, url, sha256, size: asset["size"].as_u64().unwrap_or(0) }))
}

fn fetch_latest_from(api_base: &str, slug: &str, current: &str, trusted: &[&str]) -> Result<Option<Release>, String> {
    let url = format!("{api_base}/repos/{slug}/releases/latest");
    let mut resp = match agent(20).get(&url).header("User-Agent", UA).header("Accept", "application/vnd.github+json").call() {
        Ok(r) => r,
        Err(ureq::Error::StatusCode(404)) => return Ok(None), // ainda nao ha Release publicado
        Err(e) => return Err(net_error(&e)),
    };
    let v: Value = resp.body_mut().read_json().map_err(|e| format!("Resposta inválida do GitHub: {e}"))?;
    parse_release(&v, current, trusted)
}

/// Consulta o ultimo Release. `Ok(None)` = ja esta na versao mais recente.
pub fn fetch_latest() -> Result<Option<Release>, String> {
    let slug = repo_slug().ok_or("O repositório de atualizações não está configurado.")?;
    fetch_latest_from(API_BASE, &slug, CURRENT, TRUSTED_DOWNLOADS)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Baixa o instalador para `dir`, conferindo o SHA-256. `progress(baixado, total)`.
fn download_to(dir: &Path, rel: &Release, mut progress: impl FnMut(u64, u64)) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Não foi possível criar a pasta temporária: {e}"))?;
    let name = format!("YTMusic-Setup-{}.exe", rel.version);
    let part = dir.join(format!("{name}.part"));
    let target = dir.join(&name);

    let mut resp = agent(900).get(&rel.url).header("User-Agent", UA).call().map_err(|e| net_error(&e))?;
    let total = resp.body().content_length().unwrap_or(rel.size);
    let mut reader = resp.body_mut().with_config().limit(MAX_DOWNLOAD).reader();

    let result = (|| -> Result<String, String> {
        let mut file = std::fs::File::create(&part).map_err(|e| format!("Não foi possível gravar o instalador: {e}"))?;
        let mut hasher = Sha256::new();
        let (mut buf, mut done, mut last) = (vec![0u8; 64 * 1024], 0u64, Instant::now());
        loop {
            let n = reader.read(&mut buf).map_err(|e| format!("Falha ao baixar: {e}"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| format!("Não foi possível gravar o instalador: {e}"))?;
            done += n as u64;
            if last.elapsed() >= Duration::from_millis(120) {
                progress(done, total);
                last = Instant::now();
            }
        }
        file.flush().map_err(|e| e.to_string())?;
        progress(done, total.max(done));
        if done == 0 {
            return Err("O download veio vazio.".into());
        }
        Ok(hex(&hasher.finalize()))
    })();

    let got = match result {
        Ok(h) => h,
        Err(e) => {
            let _ = std::fs::remove_file(&part);
            return Err(e);
        }
    };
    if got != rel.sha256 {
        let _ = std::fs::remove_file(&part);
        return Err("O arquivo baixado não confere com o SHA-256 publicado. Atualização cancelada por segurança.".into());
    }
    let _ = std::fs::remove_file(&target);
    std::fs::rename(&part, &target).map_err(|e| format!("Não foi possível finalizar o download: {e}"))?;
    Ok(target)
}

/// Baixa e verifica o instalador numa pasta temporaria propria do app.
pub fn download(rel: &Release, progress: impl FnMut(u64, u64)) -> Result<PathBuf, String> {
    download_to(&std::env::temp_dir().join("ytmusic-update"), rel, progress)
}

/// Executa o instalador em modo silencioso e destacado do app (que deve fechar logo em seguida).
pub fn launch_installer(path: &Path) -> Result<(), String> {
    let mut cmd = std::process::Command::new(path);
    cmd.args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0008 | 0x0000_0200); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
    cmd.spawn().map(|_| ()).map_err(|e| format!("Não foi possível iniciar o instalador: {e}"))
}

// ---------------------------------------------------------------- configuracoes

fn yes() -> bool {
    true
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Settings {
    /// Verificar sozinho uma vez por dia.
    #[serde(default = "yes")]
    pub auto_check: bool,
    /// Quando foi a ultima verificacao concluida (segundos Unix).
    #[serde(default)]
    pub last_check: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { auto_check: true, last_check: 0 }
    }
}

impl Settings {
    fn file() -> PathBuf {
        util::data_dir().join("settings.json")
    }

    pub fn load() -> Settings {
        std::fs::read_to_string(Self::file()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(Self::file(), s);
        }
    }

    /// Ja passou um dia desde a ultima verificacao (e a verificacao automatica esta ligada)?
    pub fn due(&self, now: u64) -> bool {
        self.auto_check && now.saturating_sub(self.last_check) >= CHECK_EVERY_SECS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader};
    use std::net::TcpListener;

    const HASH_A: &str = "90049059d9e269d6a696d00d8ae3ba150d63a97737df4a047c3a1d272e6e208c";
    const HASH_B: &str = "c84cc96ebe9ab3f7c24ec0a4f64cf9062099b32739ed4a000c3caf3bf8e8d9da";

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(parse_version("v1.2.3"), Some([1, 2, 3]));
        assert_eq!(parse_version("0.10"), Some([0, 10, 0]));
        assert_eq!(parse_version("1.2.3-beta.1"), Some([1, 2, 3]));
        assert_eq!(parse_version("abc"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("v0.10.0", "0.9.9"), "0.10 > 0.9, nao e comparacao de texto");
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.0.9", "0.1.0"));
        assert!(!is_newer("lixo", "0.1.0"));
    }

    #[test]
    fn repo_slug_comes_from_cargo_toml() {
        let slug = repo_slug().expect("repository configurado");
        assert_eq!(slug.matches('/').count(), 1);
    }

    #[test]
    fn sha256_is_found_in_release_notes() {
        let notes = format!("Instalador.\n\nSHA-256: `{HASH_A}`\n");
        assert_eq!(extract_sha256(&notes).as_deref(), Some(HASH_A));
        assert_eq!(extract_sha256(&format!("sha256 {}", HASH_A.to_uppercase())).as_deref(), Some(HASH_A));
        assert_eq!(extract_sha256("sem hash aqui"), None);
        assert_eq!(extract_sha256("SHA-256: abc123"), None, "curto demais");
        assert_eq!(extract_sha256(&format!("SHA-256: {HASH_A}00")), None, "longo demais");
    }

    fn release_json(tag: &str, url: &str, digest: Option<&str>, notes: &str) -> Value {
        let mut asset = serde_json::json!({
            "name": format!("YTMusic-Setup-{}.exe", tag.trim_start_matches('v')),
            "browser_download_url": url,
            "size": 3,
        });
        if let Some(d) = digest {
            asset["digest"] = Value::String(format!("sha256:{d}"));
        }
        serde_json::json!({ "tag_name": tag, "body": notes, "assets": [asset] })
    }

    #[test]
    fn parse_release_rules() {
        let url = "https://github.com/o/r/releases/download/v0.2.0/YTMusic-Setup-0.2.0.exe";
        let trusted = TRUSTED_DOWNLOADS;
        // mais nova, hash pelo digest
        let r = parse_release(&release_json("v0.2.0", url, Some(HASH_A), ""), "0.1.0", trusted).unwrap().unwrap();
        assert_eq!((r.version.as_str(), r.sha256.as_str(), r.size), ("0.2.0", HASH_A, 3));
        // mais nova, hash so pelas notas
        let notes = format!("SHA-256: `{HASH_A}`");
        assert!(parse_release(&release_json("v0.2.0", url, None, &notes), "0.1.0", trusted).unwrap().is_some());
        // mesma versao: nada a fazer
        assert_eq!(parse_release(&release_json("v0.1.0", url, Some(HASH_A), ""), "0.1.0", trusted).unwrap(), None);
        // hashes divergentes entre digest e notas: recusa
        let diverge = format!("SHA-256: {HASH_B}");
        assert!(parse_release(&release_json("v0.2.0", url, Some(HASH_A), &diverge), "0.1.0", trusted).is_err());
        // sem hash nenhum: recusa
        assert!(parse_release(&release_json("v0.2.0", url, None, "sem hash"), "0.1.0", trusted).is_err());
        // endereco fora do GitHub: recusa
        let evil = "https://example.com/YTMusic-Setup-0.2.0.exe";
        assert!(parse_release(&release_json("v0.2.0", evil, Some(HASH_A), ""), "0.1.0", trusted).is_err());
        // Release sem o instalador
        let mut j = release_json("v0.2.0", url, Some(HASH_A), "");
        j["assets"] = serde_json::json!([]);
        assert!(parse_release(&j, "0.1.0", trusted).is_err());
    }

    #[test]
    fn settings_due_once_a_day() {
        let mut s = Settings { auto_check: true, last_check: 1_000 };
        assert!(!s.due(1_000 + CHECK_EVERY_SECS - 1));
        assert!(s.due(1_000 + CHECK_EVERY_SECS));
        s.auto_check = false;
        assert!(!s.due(u64::MAX), "desligado nunca verifica");
        let parsed: Settings = serde_json::from_str("{}").unwrap();
        assert!(parsed.auto_check && parsed.last_check == 0, "campos ausentes usam os padroes");
    }

    /// Servidor HTTP minimo em 127.0.0.1 para testar o fluxo de ponta a ponta sem rede.
    fn serve(routes: Vec<(&'static str, Vec<u8>)>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let mut reader = BufReader::new(&stream);
                    let mut first = String::new();
                    reader.read_line(&mut first).ok();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                    }
                    let path = first.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let mut out = &stream;
                    match routes.iter().find(|(p, _)| *p == path) {
                        Some((_, body)) => {
                            let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                            let _ = out.write_all(head.as_bytes());
                            let _ = out.write_all(body);
                        }
                        None => {
                            let _ = out.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                        }
                    }
                });
            }
        });
        base
    }

    #[test]
    fn end_to_end_check_and_download_against_a_local_server() {
        let installer = b"fake installer bytes".to_vec();
        let good = hex(&Sha256::digest(&installer));
        let trusted = ["http://127.0.0.1"];

        // O JSON precisa conhecer a porta do servidor de arquivos: sobe esse primeiro e depois o da API.
        let base = serve(vec![("/asset", installer.clone())]);
        let json = release_json("v99.0.0", &format!("{base}/asset"), Some(&good), "").to_string().into_bytes();
        let api_base = serve(vec![("/repos/o/r/releases/latest", json)]);
        let bad_json = release_json("v99.0.0", &format!("{base}/asset"), Some(HASH_A), "").to_string().into_bytes();
        let bad_api = serve(vec![("/repos/o/r/releases/latest", bad_json)]);

        // Consulta + download com hash correto
        let rel = fetch_latest_from(&api_base, "o/r", "0.1.0", &trusted).unwrap().expect("ha versao nova");
        assert_eq!(rel.version, "99.0.0");
        let dir = std::env::temp_dir().join(format!("ytmusic-updater-test-{}", std::process::id()));
        let mut last = (0, 0);
        let path = download_to(&dir, &rel, |d, t| last = (d, t)).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), installer);
        assert_eq!(last.0, installer.len() as u64);
        assert!(!dir.join("YTMusic-Setup-99.0.0.exe.part").exists());

        // Hash publicado errado: o download e descartado e nada fica no disco
        let _ = std::fs::remove_dir_all(&dir);
        let rel_bad = fetch_latest_from(&bad_api, "o/r", "0.1.0", &trusted).unwrap().unwrap();
        let err = download_to(&dir, &rel_bad, |_, _| {}).unwrap_err();
        assert!(err.contains("SHA-256"), "{err}");
        assert!(!dir.join("YTMusic-Setup-99.0.0.exe").exists());
        assert!(!dir.join("YTMusic-Setup-99.0.0.exe.part").exists());

        // Servidor sem Release (404) = nada a atualizar
        let empty = serve(vec![]);
        assert_eq!(fetch_latest_from(&empty, "o/r", "0.1.0", &trusted).unwrap(), None);
        // Ja na versao mais nova
        assert_eq!(fetch_latest_from(&api_base, "o/r", "99.0.0", &trusted).unwrap(), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
