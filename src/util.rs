use std::path::PathBuf;

/// Pasta de dados do app: `%LOCALAPPDATA%\ytmusic`.
pub fn data_dir() -> PathBuf {
    let dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("ytmusic");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Cache de audio ja baixado.
pub fn cache_dir() -> PathBuf {
    let dir = data_dir().join("cache");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// User-Agent de navegador com a versao real do Edge/WebView2 instalado. Um UA
/// muito antigo (o anterior era fixo no Chrome 130) faz o Google desconfiar.
pub fn browser_ua() -> &'static str {
    static UA: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    UA.get_or_init(|| {
        let major = edge_version()
            .and_then(|v| v.split('.').next().map(str::to_string))
            .filter(|m| m.parse::<u32>().is_ok_and(|n| n >= 120))
            .unwrap_or_else(|| "142".to_string());
        format!(
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/{major}.0.0.0 Safari/537.36 Edg/{major}.0.0.0"
        )
    })
}

/// Versao do WebView2 Runtime (ou do Edge), lida do registro do EdgeUpdate.
#[cfg(windows)]
fn edge_version() -> Option<String> {
    use windows::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
    use windows::core::{HSTRING, w};
    const CLIENTS: [&str; 2] = [
        "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}", // WebView2 Runtime
        "{56EB18F8-B008-4CBD-B6D2-8C97FE7E9062}", // Edge
    ];
    let roots: [(HKEY, &str); 2] = [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\"),
        (HKEY_CURRENT_USER, r"Software\Microsoft\EdgeUpdate\Clients\"),
    ];
    for client in CLIENTS {
        for (root, base) in roots {
            let key = HSTRING::from(format!("{base}{client}"));
            let mut buf = [0u16; 64];
            let mut len = (buf.len() * 2) as u32;
            let ok = unsafe {
                RegGetValueW(root, &key, w!("pv"), RRF_RT_REG_SZ, None, Some(buf.as_mut_ptr().cast()), Some(&mut len))
            };
            if ok.is_ok() {
                let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                let v = String::from_utf16_lossy(&buf[..n]);
                if !v.is_empty() && v != "0.0.0.0" {
                    return Some(v);
                }
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn edge_version() -> Option<String> {
    None
}

pub fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Data UTC de hoje como `YYYYMMDD`, usada na versao do cliente InnerTube.
pub fn today_yyyymmdd() -> String {
    let days = (unix_secs() / 86_400) as i64;
    // Algoritmo de Howard Hinnant (civil_from_days).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}{m:02}{d:02}")
}

/// Formata segundos como `m:ss` (ou `h:mm:ss`).
pub fn fmt_time(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_time() {
        assert_eq!(fmt_time(5), "0:05");
        assert_eq!(fmt_time(225), "3:45");
        assert_eq!(fmt_time(3725), "1:02:05");
    }

    #[test]
    fn ua_is_recent() {
        let ua = browser_ua();
        let major: u32 = ua.split("Chrome/").nth(1).unwrap().split('.').next().unwrap().parse().unwrap();
        assert!(major >= 120, "{ua}");
    }

    #[test]
    fn date_has_eight_digits() {
        let d = today_yyyymmdd();
        assert_eq!(d.len(), 8);
        assert!(d.starts_with("20"));
    }
}
