//! Twitch: portada de directos, búsqueda, canales y grabaciones. El catálogo sale de la API GQL de
//! Twitch y el video, de `usher` (HLS). Portado de `Sanson-FLinux/plugins/twitch/script.js`.
//!
//! Se usa el cliente web de Twitch, sin cuenta. `supported_codecs=h264` evita las variantes HEVC y
//! AV1 que el procesador no decodifica. Los cortes publicitarios de los directos no se quitan acá:
//! la fuente lleva `twitch_ads` y el proxy de playlists los saca (ver `playlist::strip_twitch_ads`).
//!
//! Los datos del JSON se leen en funciones aparte, para probarlas sin red.

use media_lite_core::http::{self, Agent};
use media_lite_core::{
    Caps, Details, Error, ErrorKind, Item, Kind, Page, Platform, Result, Season, Section, Source,
};
use serde_json::{json, Value};

use crate::util::{jarr, jstr, now, pct_encode};
use crate::Provider;

const GQL: &str = "https://gql.twitch.tv/gql";
const USHER: &str = "https://usher.ttvnw.net/";
/// Id del cliente web de Twitch (el mismo que usan Sanson y streamlink).
const CLIENT_ID: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";
const SITE: &str = "https://www.twitch.tv";
const REFERER: &str = "https://www.twitch.tv/";
const PAGE: u64 = 30;
const MAX_JSON: u64 = 4 * 1024 * 1024;
const TOKEN_PARAMS: &str =
    "params:{platform:\"web\",playerBackend:\"mediaplayer\",playerType:\"site\"}";
const STREAM: &str =
    "stream{id title viewersCount createdAt previewImageURL(width:320,height:180) game{displayName}}";
const USER: &str = "id login displayName profileImageURL(width:150) followers{totalCount}";
const VIDEO: &str = "id title lengthSeconds viewCount createdAt publishedAt previewThumbnailURL(width:320,height:180) broadcastType owner{id login displayName profileImageURL(width:70)} game{displayName}";

pub struct Twitch {
    agent: Agent,
}

impl Default for Twitch {
    fn default() -> Twitch {
        Twitch::new()
    }
}

impl Twitch {
    pub fn new() -> Twitch {
        Twitch {
            agent: http::agent(),
        }
    }

    /// Pedido GQL. Un error de la API sin datos es un `Error`; con datos parciales, se usan.
    fn gql(&self, query: &str, variables: Value) -> Result<Value> {
        let body = json!({ "query": query, "variables": variables }).to_string();
        let headers = [
            ("Client-Id", CLIENT_ID),
            ("Content-Type", "application/json"),
            ("Origin", SITE),
            ("Referer", REFERER),
        ];
        let response = http::request(&self.agent, "POST", GQL, &headers, Some(&body), MAX_JSON)?;
        if !response.ok() {
            return Err(Error::network(format!(
                "Twitch respondió con error {}",
                response.status
            )));
        }
        parse_gql(&response.body)
    }

    fn top_streams(&self, cursor: Option<&str>) -> Result<Page> {
        const QUERY: &str = "query($n:Int,$a:Cursor){streams(first:$n,after:$a){edges{cursor node{id title viewersCount createdAt previewImageURL(width:320,height:180) game{displayName} broadcaster{id login displayName profileImageURL(width:70)}}} pageInfo{hasNextPage}}}";
        let data = self.gql(QUERY, json!({ "n": PAGE, "a": cursor }))?;
        Ok(streams_page(&data["streams"]))
    }

    fn search_all(&self, query: &str) -> Result<Vec<Item>> {
        const SEARCH: &str = "query($q:String!){searchFor(userQuery:$q,platform:\"web\",options:{targets:[{index:CHANNEL},{index:VOD}]}){channels{edges{item{...on User{@USER@ @STREAM@}}}} videos{edges{item{...on Video{@VIDEO@}}}}}}";
        let data = self.gql(&gql_query(SEARCH), json!({ "q": query }))?;
        Ok(search_items(&data["searchFor"]))
    }

    fn channel(&self, login: &str) -> Result<Value> {
        const CHANNEL: &str = "query($l:String!,$n:Int){user(login:$l){@USER@ @STREAM@ videos(first:$n,sort:TIME){edges{node{@VIDEO@}}}}}";
        let data = self.gql(&gql_query(CHANNEL), json!({ "l": login, "n": PAGE }))?;
        let user = data["user"].clone();
        if user.is_null() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                format!("El canal no existe: {login}"),
            ));
        }
        Ok(user)
    }

    fn live_details(&self, login: &str) -> Result<Details> {
        const LIVE: &str = "query($l:String!){user(login:$l){@USER@ description @STREAM@} streamPlaybackAccessToken(channelName:$l,@TOKEN@){value signature}}";
        let data = self.gql(&gql_query(LIVE), json!({ "l": login }))?;
        let user = &data["user"];
        if user.is_null() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                format!("El canal no existe: {login}"),
            ));
        }
        let name = user_name(user);
        let stream = &user["stream"];
        if stream.is_null() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                format!("{name} no está en vivo ahora."),
            ));
        }
        let token = &data["streamPlaybackAccessToken"];
        if let Some(reason) = forbidden_reason(token) {
            return Err(Error::new(ErrorKind::Unavailable, reason));
        }
        if token.is_null() {
            return Err(Error::network("Twitch no entregó el directo"));
        }
        let item = live_item(user, stream);
        let description = match game(stream) {
            "" => jstr(user, "description"),
            g => format!("Jugando: {g}\n\n{}", jstr(user, "description")),
        };
        Ok(Details {
            item,
            description,
            sources: vec![Source {
                url: usher_url(&format!("api/channel/hls/{login}.m3u8"), token),
                label: "Directo".into(),
                height: None,
                live: true,
                audio_url: None,
                drop: Vec::new(),
                twitch_ads: true,
                available_at: None,
            }],
        })
    }

    fn vod_details(&self, id: &str) -> Result<Details> {
        const VOD: &str = "query($id:ID!){video(id:$id){@VIDEO@ description} videoPlaybackAccessToken(id:$id,@TOKEN@){value signature}}";
        let data = self.gql(&gql_query(VOD), json!({ "id": id }))?;
        let video = &data["video"];
        let token = &data["videoPlaybackAccessToken"];
        if video.is_null() || token.is_null() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                "La grabación no existe o es solo para suscriptores.",
            ));
        }
        if let Some(reason) = forbidden_reason(token) {
            return Err(Error::new(ErrorKind::Unavailable, reason));
        }
        Ok(Details {
            item: vod_item(video, &video["owner"]),
            description: jstr(video, "description"),
            sources: vec![Source {
                url: usher_url(&format!("vod/{id}.m3u8"), token),
                label: "Grabación".into(),
                height: None,
                live: false,
                audio_url: None,
                drop: Vec::new(),
                twitch_ads: false,
                available_at: None,
            }],
        })
    }
}

impl Provider for Twitch {
    fn platform(&self) -> Platform {
        Platform::Twitch
    }

    fn caps(&self) -> Caps {
        Caps {
            live: true,
            search: true,
            ..Caps::default()
        }
    }

    fn list(
        &self,
        section: Section,
        _category: Option<&str>,
        cursor: Option<&str>,
    ) -> Result<Page> {
        match section {
            Section::Live => self.top_streams(cursor),
            _ => Err(Error::unsupported()),
        }
    }

    fn search(&self, query: &str, _cursor: Option<&str>) -> Result<Page> {
        Ok(Page {
            items: self.search_all(query)?,
            next: None,
        })
    }

    fn details(&self, item: &Item) -> Result<Details> {
        match item.kind {
            Kind::Live => self.live_details(&item.id),
            Kind::Video => self.vod_details(&item.id),
            _ => Err(Error::unsupported()),
        }
    }

    /// Un canal se abre como una serie: «En vivo» (si está en directo) y «Transmisiones anteriores».
    /// Los canales seguidos que están en directo ahora (una sola consulta, hasta 100 canales).
    fn feed(&self, channels: &[Item]) -> Result<Vec<Item>> {
        let logins: Vec<String> = channels
            .iter()
            .filter(|c| c.kind == Kind::Series)
            .take(100)
            .map(|c| c.id.clone())
            .collect();
        if logins.is_empty() {
            return Ok(Vec::new());
        }
        const FEED: &str = "query($l:[String!]){users(logins:$l){@USER@ @STREAM@}}";
        let data = self.gql(&gql_query(FEED), json!({ "l": logins }))?;
        Ok(jarr(&data, "users")
            .iter()
            .filter(|u| u["stream"].is_object())
            .map(|u| live_item(u, &u["stream"]))
            .collect())
    }

    fn seasons(&self, channel: &Item) -> Result<Vec<Season>> {
        if channel.kind != Kind::Series {
            return Err(Error::unsupported());
        }
        let user = self.channel(&channel.id)?;
        let seasons = channel_seasons(&user);
        if seasons.is_empty() {
            return Err(Error::new(
                ErrorKind::Unavailable,
                format!(
                    "{} no está en vivo ni tiene transmisiones guardadas.",
                    user_name(&user)
                ),
            ));
        }
        Ok(seasons)
    }
}

/// Lee la respuesta de GQL: el campo `data`, o el primer error si no hay datos.
pub fn parse_gql(body: &[u8]) -> Result<Value> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| Error::parse("Twitch: la respuesta no se entiende"))?;
    let data = value["data"].clone();
    if data.is_null() {
        let message = value["errors"][0]["message"]
            .as_str()
            .unwrap_or("sin datos");
        return Err(Error::new(ErrorKind::Other, format!("Twitch: {message}")));
    }
    Ok(data)
}

/// Nombre para mostrar de un usuario, o su login si no trae nombre.
fn user_name(user: &Value) -> String {
    let name = jstr(user, "displayName");
    if name.is_empty() {
        jstr(user, "login")
    } else {
        name
    }
}

fn game(stream: &Value) -> &str {
    stream["game"]["displayName"].as_str().unwrap_or("")
}

/// Une los textos no vacíos con « · ».
fn dotted(parts: &[&str]) -> String {
    parts
        .iter()
        .filter(|p| !p.is_empty())
        .copied()
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Rellena los marcadores de una consulta GQL con sus campos.
fn gql_query(template: &str) -> String {
    template
        .replace("@USER@", USER)
        .replace("@STREAM@", STREAM)
        .replace("@VIDEO@", VIDEO)
        .replace("@TOKEN@", TOKEN_PARAMS)
}

fn non_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

/// Un directo. `user` trae el login y el nombre; `stream`, el título, la miniatura y el juego.
pub fn live_item(user: &Value, stream: &Value) -> Item {
    let login = jstr(user, "login");
    let title = jstr(stream, "title");
    let mut item = Item::new(
        Platform::Twitch,
        Kind::Live,
        login,
        if title.is_empty() {
            user_name(user)
        } else {
            title
        },
    );
    item.subtitle = dotted(&[&user_name(user), game(stream)]);
    item.thumb = non_empty(jstr(stream, "previewImageURL"));
    item.live = true;
    if let Some(viewers) = stream["viewersCount"].as_u64() {
        item.description = format!("{viewers} espectadores");
    }
    item
}

/// Una grabación. El dueño es `owner` o, si no viene, `fallback` (el canal que se está mirando).
pub fn vod_item(video: &Value, fallback: &Value) -> Item {
    let owner = if video["owner"].is_object() {
        &video["owner"]
    } else {
        fallback
    };
    let mut item = Item::new(
        Platform::Twitch,
        Kind::Video,
        jstr(video, "id"),
        jstr(video, "title"),
    );
    item.subtitle = user_name(owner);
    item.thumb = non_empty(jstr(video, "previewThumbnailURL"));
    item.duration_secs = video["lengthSeconds"].as_u64().filter(|s| *s > 0);
    item.description = match video["viewCount"].as_u64() {
        Some(views) => format!("{views} vistas"),
        None => String::new(),
    };
    item
}

/// Un canal.
pub fn channel_item(user: &Value) -> Item {
    // Un canal es una serie: sus temporadas («En vivo», «Transmisiones anteriores») las abre la
    // interfaz como las de cualquier serie. `Kind::Channel` queda para los canales de TV.
    let mut item = Item::new(
        Platform::Twitch,
        Kind::Series,
        jstr(user, "login"),
        user_name(user),
    );
    item.thumb = non_empty(jstr(user, "profileImageURL"));
    item.subtitle = match user["followers"]["totalCount"].as_u64() {
        Some(n) => format!("{n} seguidores"),
        None => String::new(),
    };
    item
}

/// La portada: los directos de `streams` y el cursor de la página siguiente.
pub fn streams_page(streams: &Value) -> Page {
    let edges = jarr(streams, "edges");
    let items = edges
        .iter()
        .filter(|e| e["node"]["broadcaster"].is_object())
        .map(|e| live_item(&e["node"]["broadcaster"], &e["node"]))
        .collect();
    let has_more = streams["pageInfo"]["hasNextPage"]
        .as_bool()
        .unwrap_or(false);
    let next = edges
        .last()
        .and_then(|e| e["cursor"].as_str())
        .filter(|_| has_more)
        .map(str::to_string);
    Page { items, next }
}

/// La búsqueda: hasta cuatro canales, después los que están en directo y por último las
/// grabaciones, como Sanson.
pub fn search_items(search: &Value) -> Vec<Item> {
    let users: Vec<&Value> = jarr(&search["channels"], "edges")
        .iter()
        .map(|e| &e["item"])
        .filter(|u| !jstr(u, "login").is_empty())
        .collect();
    let mut out: Vec<Item> = users.iter().take(4).map(|u| channel_item(u)).collect();
    out.extend(
        users
            .iter()
            .filter(|u| u["stream"].is_object())
            .map(|u| live_item(u, &u["stream"])),
    );
    out.extend(
        jarr(&search["videos"], "edges")
            .iter()
            .map(|e| &e["item"])
            .filter(|v| !jstr(v, "id").is_empty())
            .map(|v| vod_item(v, &Value::Null)),
    );
    out
}

/// Las temporadas de un canal: «En vivo» si está en directo, y «Transmisiones anteriores».
pub fn channel_seasons(user: &Value) -> Vec<Season> {
    let mut seasons = Vec::new();
    if user["stream"].is_object() {
        seasons.push(Season {
            number: 1,
            name: "En vivo".into(),
            episodes: vec![live_item(user, &user["stream"])],
        });
    }
    let vods: Vec<Item> = jarr(&user["videos"], "edges")
        .iter()
        .map(|e| vod_item(&e["node"], user))
        .collect();
    if !vods.is_empty() {
        seasons.push(Season {
            number: seasons.len() as u32 + 1,
            name: "Transmisiones anteriores".into(),
            episodes: vods,
        });
    }
    seasons
}

/// Si el token dice que el contenido no se puede ver, el motivo.
pub fn forbidden_reason(token: &Value) -> Option<String> {
    let value: Value = serde_json::from_str(token["value"].as_str()?).ok()?;
    let auth = &value["authorization"];
    if auth["forbidden"].as_bool() != Some(true) {
        return None;
    }
    Some(
        auth["reason"]
            .as_str()
            .filter(|r| !r.is_empty())
            .unwrap_or("No disponible")
            .to_string(),
    )
}

/// URL de `usher` con el token firmado. `supported_codecs=h264` deja fuera HEVC y AV1.
pub fn usher_url(path: &str, token: &Value) -> String {
    let p = now() % 9_000_000 + 1_000_000;
    let params: [(&str, String); 12] = [
        ("acmb", "e30=".into()),
        ("allow_source", "true".into()),
        ("fast_bread", "true".into()),
        ("p", p.to_string()),
        ("player_backend", "mediaplayer".into()),
        ("playlist_include_framerate", "true".into()),
        ("reassignments_supported", "true".into()),
        ("sig", jstr(token, "signature")),
        ("supported_codecs", "h264".into()),
        ("token", jstr(token, "value")),
        ("transcode_mode", "cbr_v1".into()),
        ("cdm", "wv".into()),
    ];
    let query: Vec<String> = params
        .iter()
        .map(|(k, v)| format!("{k}={}", pct_encode(v)))
        .collect();
    format!("{USHER}{path}?{}", query.join("&"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user() -> Value {
        json!({
            "id": "1", "login": "ninja", "displayName": "Ninja",
            "profileImageURL": "https://img/ninja.png", "followers": { "totalCount": 1200 },
            "stream": {
                "id": "9", "title": "Ranked", "viewersCount": 5000,
                "previewImageURL": "https://img/p.jpg", "game": { "displayName": "Fortnite" }
            }
        })
    }

    #[test]
    fn live_item_has_login_as_id_and_game_in_subtitle() {
        let u = user();
        let item = live_item(&u, &u["stream"]);
        assert_eq!(item.kind, Kind::Live);
        assert_eq!(item.id, "ninja");
        assert_eq!(item.title, "Ranked");
        assert_eq!(item.subtitle, "Ninja · Fortnite");
        assert!(item.live);
        assert_eq!(item.description, "5000 espectadores");
    }

    #[test]
    fn channel_seasons_put_the_live_stream_first() {
        let mut u = user();
        u["videos"] = json!({ "edges": [
            { "node": { "id": "77", "title": "VOD", "lengthSeconds": 3600, "owner": u.clone() } }
        ]});
        let seasons = channel_seasons(&u);
        assert_eq!(seasons.len(), 2);
        assert_eq!(seasons[0].name, "En vivo");
        assert_eq!(seasons[1].name, "Transmisiones anteriores");
        assert_eq!(seasons[1].episodes[0].kind, Kind::Video);
        assert_eq!(seasons[1].episodes[0].duration_secs, Some(3600));
    }

    #[test]
    fn offline_channel_without_videos_has_no_seasons() {
        let u = json!({ "login": "x", "displayName": "X", "videos": { "edges": [] } });
        assert!(channel_seasons(&u).is_empty());
    }

    #[test]
    fn search_lists_channels_then_lives_then_videos() {
        let u = user();
        let search = json!({
            "channels": { "edges": [ { "item": u } ] },
            "videos": { "edges": [ { "item": { "id": "42", "title": "Mejores jugadas", "owner": { "displayName": "Ninja" } } } ] }
        });
        let items = search_items(&search);
        let kinds: Vec<Kind> = items.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, vec![Kind::Series, Kind::Live, Kind::Video]);
    }

    #[test]
    fn streams_page_uses_the_last_cursor_only_when_there_is_more() {
        let page = json!({
            "edges": [
                { "cursor": "c1", "node": { "title": "A", "broadcaster": { "login": "a", "displayName": "A" } } },
                { "cursor": "c2", "node": { "title": "B", "broadcaster": { "login": "b", "displayName": "B" } } }
            ],
            "pageInfo": { "hasNextPage": true }
        });
        let p = streams_page(&page);
        assert_eq!(p.items.len(), 2);
        assert_eq!(p.next.as_deref(), Some("c2"));

        let last = json!({ "edges": [], "pageInfo": { "hasNextPage": false } });
        assert_eq!(streams_page(&last).next, None);
    }

    #[test]
    fn forbidden_token_gives_the_reason() {
        let token = json!({ "value": "{\"authorization\":{\"forbidden\":true,\"reason\":\"geo\"}}", "signature": "s" });
        assert_eq!(forbidden_reason(&token).as_deref(), Some("geo"));
        let ok = json!({ "value": "{\"authorization\":{\"forbidden\":false}}", "signature": "s" });
        assert_eq!(forbidden_reason(&ok), None);
    }

    #[test]
    fn usher_asks_for_h264_and_encodes_the_token() {
        let token = json!({ "value": "{\"a\":\"b c\"}", "signature": "abc123" });
        let url = usher_url("api/channel/hls/ninja.m3u8", &token);
        assert!(url.starts_with("https://usher.ttvnw.net/api/channel/hls/ninja.m3u8?"));
        assert!(url.contains("supported_codecs=h264"));
        assert!(url.contains("sig=abc123"));
        assert!(url.contains("token=%7B%22a%22%3A%22b%20c%22%7D"));
    }

    #[test]
    fn gql_templates_are_balanced_after_filling() {
        const TEMPLATES: [&str; 4] = [
            "query($q:String!){searchFor(userQuery:$q,platform:\"web\",options:{targets:[{index:CHANNEL},{index:VOD}]}){channels{edges{item{...on User{@USER@ @STREAM@}}}} videos{edges{item{...on Video{@VIDEO@}}}}}}",
            "query($l:String!,$n:Int){user(login:$l){@USER@ @STREAM@ videos(first:$n,sort:TIME){edges{node{@VIDEO@}}}}}",
            "query($l:String!){user(login:$l){@USER@ description @STREAM@} streamPlaybackAccessToken(channelName:$l,@TOKEN@){value signature}}",
            "query($id:ID!){video(id:$id){@VIDEO@ description} videoPlaybackAccessToken(id:$id,@TOKEN@){value signature}}",
        ];
        for t in TEMPLATES {
            let q = gql_query(t);
            assert!(!q.contains('@'), "{q}");
            assert_eq!(q.matches('{').count(), q.matches('}').count(), "{q}");
        }
    }

    #[test]
    fn gql_errors_without_data_are_errors() {
        let err = parse_gql(br#"{"errors":[{"message":"boom"}],"data":null}"#).unwrap_err();
        assert!(err.message.contains("boom"));
        assert!(parse_gql(br#"{"data":{"user":null}}"#).is_ok());
    }
}
