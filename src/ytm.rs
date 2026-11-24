//! Cliente minimo da API interna do YouTube Music (InnerTube, a mesma que o
//! site usa). Nao e uma API oficial: o formato das respostas pode mudar, por
//! isso o parser procura os blocos pelo nome em vez de seguir caminhos fixos.

use crate::auth::{Auth, ORIGIN};
use crate::util;
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Track {
    pub video_id: String,
    pub title: String,
    pub artists: String,
    pub album: String,
    pub duration_secs: u32,
    pub thumb: String,
}

#[derive(Clone, Debug, Default)]
pub struct Playlist {
    pub browse_id: String,
    pub title: String,
    pub subtitle: String,
    pub thumb: String,
}

#[derive(Clone)]
pub struct Client {
    auth: Option<Auth>,
    agent: ureq::Agent,
}

const SONGS_FILTER: &str = "EgWKAQIIAWoMEA4QChADEAQQCRAF";

impl Client {
    pub fn new(auth: Option<Auth>) -> Client {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(20)))
            .build();
        Client {
            auth,
            agent: config.into(),
        }
    }

    pub fn is_logged_in(&self) -> bool {
        self.auth.is_some()
    }

    fn post(&self, endpoint: &str, mut body: Value) -> Result<Value, String> {
        body["context"] = json!({
            "client": {
                "clientName": "WEB_REMIX",
                "clientVersion": format!("1.{}.01.00", util::today_yyyymmdd()),
                "hl": "pt",
            },
            "user": {},
        });
        let url = format!("{ORIGIN}/youtubei/v1/{endpoint}?prettyPrint=false");
        let mut req = self
            .agent
            .post(&url)
            .header("User-Agent", util::browser_ua())
            .header("Origin", ORIGIN)
            .header("X-Goog-AuthUser", "0")
            .header("Accept-Language", "pt-BR,pt;q=0.9,en;q=0.8");
        if let Some(auth) = &self.auth {
            req = req
                .header("Cookie", &auth.cookie_header())
                .header("Authorization", &auth.authorization());
        } else {
            req = req.header("Cookie", "SOCS=CAI");
        }
        let mut resp = req.send_json(&body).map_err(|e| friendly(&e))?;
        resp.body_mut().read_json::<Value>().map_err(|e| e.to_string())
    }

    pub fn search(&self, query: &str) -> Result<Vec<Track>, String> {
        let resp = self.post("search", json!({ "query": query, "params": SONGS_FILTER }))?;
        Ok(parse_tracks(&resp))
    }

    /// Musicas curtidas ("Liked songs"). Exige conta conectada.
    pub fn liked(&self) -> Result<Vec<Track>, String> {
        self.browse_tracks("VLLM")
    }

    pub fn playlist(&self, browse_id: &str) -> Result<Vec<Track>, String> {
        self.browse_tracks(browse_id)
    }

    fn browse_tracks(&self, browse_id: &str) -> Result<Vec<Track>, String> {
        let mut resp = self.post("browse", json!({ "browseId": browse_id }))?;
        let mut tracks = parse_tracks(&resp);
        // Listas grandes vem em paginas; segue o token de continuacao.
        for _ in 0..20 {
            let Some(token) = continuation_token(&resp) else { break };
            match self.post("browse", json!({ "continuation": token })) {
                Ok(next) => {
                    let more = parse_tracks(&next);
                    if more.is_empty() {
                        break;
                    }
                    tracks.extend(more);
                    resp = next;
                }
                Err(_) => break,
            }
        }
        Ok(tracks)
    }

    pub fn library_playlists(&self) -> Result<Vec<Playlist>, String> {
        let resp = self.post("browse", json!({ "browseId": "FEmusic_liked_playlists" }))?;
        Ok(parse_playlists(&resp))
    }

    /// "Mix": fila automatica baseada numa musica (o "Iniciar radio" do app).
    pub fn mix(&self, video_id: &str) -> Result<Vec<Track>, String> {
        let resp = self.post(
            "next",
            json!({
                "videoId": video_id,
                "playlistId": format!("RDAMVM{video_id}"),
                "isAudioOnly": true,
                "tunerSettingValue": "AUTOMIX_SETTING_NORMAL",
                "enablePersistentPlaylistPanel": true,
                "watchEndpointMusicSupportedConfigs": {
                    "watchEndpointMusicConfig": {
                        "hasPersistentPlaylistPanel": true,
                        "musicVideoType": "MUSIC_VIDEO_TYPE_ATV"
                    }
                },
            }),
        )?;
        Ok(parse_queue(&resp))
    }

    /// Adiciona uma faixa a uma playlist da biblioteca. `browse_id` e o da grade
    /// de playlists (`VLPL...`); a API quer o id sem o prefixo `VL`.
    pub fn add_to_playlist(&self, browse_id: &str, video_id: &str) -> Result<(), String> {
        if self.auth.is_none() {
            return Err(crate::tr!("playlists.need_login").into());
        }
        let playlist_id = browse_id.strip_prefix("VL").unwrap_or(browse_id);
        let resp = self.post(
            "browse/edit_playlist",
            json!({
                "playlistId": playlist_id,
                "actions": [{ "action": "ACTION_ADD_VIDEO", "addedVideoId": video_id, "dedupeOption": "DEDUPE_OPTION_SKIP" }],
            }),
        )?;
        match resp.get("status").and_then(Value::as_str) {
            Some("STATUS_SUCCEEDED") => Ok(()),
            Some(other) => Err(crate::tr!("playlists.rejected", status = other)),
            None => Err(crate::tr!("playlists.not_confirmed").into()),
        }
    }

    /// Nome da conta conectada, se a API devolver.
    pub fn account_name(&self) -> Option<String> {
        if self.auth.is_none() {
            return None;
        }
        let resp = self.post("account/account_menu", json!({})).ok()?;
        let mut found = Vec::new();
        find_all(&resp, "activeAccountHeaderRenderer", &mut found);
        let header = found.first()?;
        text_of(header.pointer("/accountName")?)
    }
}

fn friendly(e: &ureq::Error) -> String {
    match e {
        ureq::Error::StatusCode(401) | ureq::Error::StatusCode(403) => {
            crate::tr!("account.session_expired").to_string()
        }
        other => crate::tr!("account.network_error", error = other),
    }
}

// ---------------------------------------------------------------- parsing

/// Coleta, em ordem, todo valor guardado sob a chave `key` em qualquer nivel.
pub(crate) fn find_all<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(map) => {
            for (k, child) in map {
                if k == key {
                    out.push(child);
                } else {
                    find_all(child, key, out);
                }
            }
        }
        Value::Array(items) => items.iter().for_each(|i| find_all(i, key, out)),
        _ => {}
    }
}

fn runs(v: &Value) -> Vec<&Value> {
    v.pointer("/runs")
        .and_then(Value::as_array)
        .map(|a| a.iter().collect())
        .unwrap_or_default()
}

fn run_text(run: &Value) -> &str {
    run.get("text").and_then(Value::as_str).unwrap_or("")
}

fn text_of(v: &Value) -> Option<String> {
    let s: String = runs(v).iter().map(|r| run_text(r)).collect();
    (!s.is_empty()).then_some(s)
}

fn page_type(run: &Value) -> &str {
    run.pointer("/navigationEndpoint/browseEndpoint/browseEndpointContextSupportedConfigs/browseEndpointContextMusicConfig/pageType")
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// `3:45` / `1:02:03` -> segundos.
pub fn parse_duration(s: &str) -> Option<u32> {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() < 2 || parts.len() > 3 {
        return None;
    }
    let mut total = 0u32;
    for p in parts {
        total = total * 60 + p.parse::<u32>().ok()?;
    }
    Some(total)
}

fn best_thumb(v: Option<&Value>) -> String {
    v.and_then(Value::as_array)
        .and_then(|a| a.last())
        .and_then(|t| t.get("url"))
        .and_then(Value::as_str)
        .map(|u| u.replace("http://", "https://"))
        .unwrap_or_default()
}

/// Separa artistas / album / duracao de uma linha de "runs" (`a • b • 3:45`).
fn split_meta(all_runs: &[&Value]) -> (String, String, u32) {
    let mut artists = Vec::new();
    let mut album = String::new();
    let mut duration = 0;
    let mut plain: Vec<String> = Vec::new();
    for r in all_runs {
        let t = run_text(r).trim();
        if t.is_empty() || t == "•" || t == "&" || t == "," {
            continue;
        }
        if let Some(d) = parse_duration(t) {
            duration = d;
            continue;
        }
        match page_type(r) {
            "MUSIC_PAGE_TYPE_ARTIST" | "MUSIC_PAGE_TYPE_USER_CHANNEL" => artists.push(t.to_string()),
            "MUSIC_PAGE_TYPE_ALBUM" | "MUSIC_PAGE_TYPE_AUDIOBOOK" => album = t.to_string(),
            _ => plain.push(t.to_string()),
        }
    }
    // Sem links de navegacao (alguns layouts): usa o texto na ordem.
    if artists.is_empty() {
        let plain: Vec<String> = plain
            .into_iter()
            .filter(|p| !matches!(p.as_str(), "Song" | "Música" | "Video" | "Vídeo"))
            .collect();
        if let Some(first) = plain.first() {
            artists.push(first.clone());
        }
        if album.is_empty() {
            if let Some(second) = plain.get(1) {
                album = second.clone();
            }
        }
    }
    (artists.join(", "), album, duration)
}

fn parse_list_item(r: &Value) -> Option<Track> {
    let video_id = r
        .pointer("/playlistItemData/videoId")
        .or_else(|| r.pointer("/overlay/musicItemThumbnailOverlayRenderer/content/musicPlayButtonRenderer/playNavigationEndpoint/watchEndpoint/videoId"))
        .and_then(Value::as_str)?
        .to_string();
    let cols = r.get("flexColumns")?.as_array()?;
    let col_runs = |i: usize| -> Vec<&Value> {
        cols.get(i)
            .and_then(|c| c.pointer("/musicResponsiveListItemFlexColumnRenderer/text"))
            .map(runs)
            .unwrap_or_default()
    };
    let title: String = col_runs(0).iter().map(|r| run_text(r)).collect();
    if title.is_empty() {
        return None;
    }
    let mut meta_runs: Vec<&Value> = Vec::new();
    for i in 1..cols.len() {
        meta_runs.extend(col_runs(i));
        // Separador entre colunas para nao colar artista com album.
    }
    let (artists, album, mut duration) = split_meta(&meta_runs);
    if duration == 0 {
        duration = r
            .pointer("/fixedColumns/0/musicResponsiveListItemFixedColumnRenderer/text/runs/0/text")
            .and_then(Value::as_str)
            .and_then(parse_duration)
            .unwrap_or(0);
    }
    Some(Track {
        video_id,
        title,
        artists,
        album,
        duration_secs: duration,
        thumb: best_thumb(r.pointer("/thumbnail/musicThumbnailRenderer/thumbnail/thumbnails")),
    })
}

fn parse_panel_item(r: &Value) -> Option<Track> {
    let video_id = r.get("videoId")?.as_str()?.to_string();
    let title = text_of(r.get("title")?)?;
    let byline = r.get("longBylineText").map(runs).unwrap_or_default();
    // longBylineText: artistas • album • ano. O ano vira "album" se nao houver link.
    let (artists, album, _) = split_meta(&byline);
    let duration = r
        .get("lengthText")
        .and_then(text_of)
        .and_then(|t| parse_duration(&t))
        .unwrap_or(0);
    Some(Track {
        video_id,
        title,
        artists,
        album: if album.chars().all(|c| c.is_ascii_digit()) { String::new() } else { album },
        duration_secs: duration,
        thumb: best_thumb(r.pointer("/thumbnail/thumbnails")),
    })
}

/// Musicas de uma resposta de busca / playlist / curtidas.
pub fn parse_tracks(resp: &Value) -> Vec<Track> {
    let mut items = Vec::new();
    find_all(resp, "musicResponsiveListItemRenderer", &mut items);
    let mut seen = std::collections::HashSet::new();
    items
        .into_iter()
        .filter_map(parse_list_item)
        .filter(|t| seen.insert(t.video_id.clone()))
        .collect()
}

/// Fila de uma resposta do endpoint `next` (mix / radio).
pub fn parse_queue(resp: &Value) -> Vec<Track> {
    let mut items = Vec::new();
    find_all(resp, "playlistPanelVideoRenderer", &mut items);
    items.into_iter().filter_map(parse_panel_item).collect()
}

pub fn parse_playlists(resp: &Value) -> Vec<Playlist> {
    let mut items = Vec::new();
    find_all(resp, "musicTwoRowItemRenderer", &mut items);
    items
        .into_iter()
        .filter_map(|r| {
            let browse_id = r
                .pointer("/navigationEndpoint/browseEndpoint/browseId")?
                .as_str()?
                .to_string();
            // "VL..." sao playlists; albuns (MPRE...) ficam de fora.
            if !browse_id.starts_with("VL") {
                return None;
            }
            Some(Playlist {
                browse_id,
                title: text_of(r.get("title")?)?,
                subtitle: r.get("subtitle").and_then(text_of).unwrap_or_default(),
                thumb: best_thumb(r.pointer("/thumbnailRenderer/musicThumbnailRenderer/thumbnail/thumbnails")),
            })
        })
        .collect()
}

fn continuation_token(resp: &Value) -> Option<String> {
    let mut found = Vec::new();
    find_all(resp, "continuationCommand", &mut found);
    if let Some(t) = found.first().and_then(|c| c.get("token")).and_then(Value::as_str) {
        return Some(t.to_string());
    }
    found.clear();
    find_all(resp, "nextContinuationData", &mut found);
    found
        .first()
        .and_then(|c| c.get("continuation"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Ajusta a URL da capa para o tamanho que a UI realmente usa.
pub fn thumb_url(url: &str, size: u32) -> String {
    // lh3.googleusercontent.com/...=w226-h226-l90-rj  ->  =w{size}-h{size}-l90-rj
    if let Some(pos) = url.rfind("=w") {
        let rest = &url[pos + 2..];
        if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            let tail = rest.find("-l").map(|i| &rest[i..]).unwrap_or("");
            return format!("{}=w{size}-h{size}{tail}", &url[..pos]);
        }
    }
    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(parse_duration("3:45"), Some(225));
        assert_eq!(parse_duration("1:02:03"), Some(3723));
        assert_eq!(parse_duration("Song"), None);
    }

    #[test]
    fn thumb_resize() {
        assert_eq!(
            thumb_url("https://lh3.googleusercontent.com/abc=w226-h226-l90-rj", 96),
            "https://lh3.googleusercontent.com/abc=w96-h96-l90-rj"
        );
        assert_eq!(thumb_url("https://i.ytimg.com/vi/x/a.jpg", 96), "https://i.ytimg.com/vi/x/a.jpg");
    }

    #[test]
    fn parses_search_item() {
        let item = json!({
          "musicResponsiveListItemRenderer": {
            "playlistItemData": {"videoId": "dQw4w9WgXcQ"},
            "thumbnail": {"musicThumbnailRenderer": {"thumbnail": {"thumbnails": [
                {"url": "https://a/1=w60-h60"}, {"url": "https://a/1=w120-h120"}]}}},
            "flexColumns": [
              {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [{"text": "Never Gonna Give You Up"}]}}},
              {"musicResponsiveListItemFlexColumnRenderer": {"text": {"runs": [
                {"text": "Rick Astley", "navigationEndpoint": {"browseEndpoint": {"browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ARTIST"}}}}},
                {"text": " • "},
                {"text": "Whenever You Need Somebody", "navigationEndpoint": {"browseEndpoint": {"browseEndpointContextSupportedConfigs": {"browseEndpointContextMusicConfig": {"pageType": "MUSIC_PAGE_TYPE_ALBUM"}}}}},
                {"text": " • "},
                {"text": "3:33"}]}}}
            ]
          }
        });
        let tracks = parse_tracks(&item);
        assert_eq!(tracks.len(), 1);
        let t = &tracks[0];
        assert_eq!(t.video_id, "dQw4w9WgXcQ");
        assert_eq!(t.artists, "Rick Astley");
        assert_eq!(t.album, "Whenever You Need Somebody");
        assert_eq!(t.duration_secs, 213);
        assert!(t.thumb.ends_with("w120-h120"));
    }

    #[test]
    fn parses_panel_item() {
        let item = json!({"playlistPanelVideoRenderer": {
            "videoId": "abc12345678",
            "title": {"runs": [{"text": "Faixa"}]},
            "longBylineText": {"runs": [{"text": "Fulano"}, {"text": " • "}, {"text": "2020"}]},
            "lengthText": {"runs": [{"text": "4:10"}]},
            "thumbnail": {"thumbnails": [{"url": "https://a/x"}]}
        }});
        let q = parse_queue(&item);
        assert_eq!(q.len(), 1);
        assert_eq!(q[0].title, "Faixa");
        assert_eq!(q[0].duration_secs, 250);
    }

    /// Chama a API de verdade (sem login). Rode com `cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn live_search() {
        let c = Client::new(None);
        let tracks = c.search("daft punk around the world").expect("busca");
        assert!(!tracks.is_empty(), "busca vazia");
        for t in tracks.iter().take(5) {
            println!("{} | {} | {} | {}s", t.video_id, t.title, t.artists, t.duration_secs);
        }
        let mix = c.mix(&tracks[0].video_id).expect("mix");
        assert!(!mix.is_empty(), "mix vazio");
    }
}
