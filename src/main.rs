use std::env;
use std::fmt;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use once_cell::sync::Lazy;
use pcg_rand::Pcg64;
use rand::distributions::Alphanumeric;
use rand::{Rng, SeedableRng};
use reqwest::{Client, ClientBuilder};
use rocket::http::{ContentType, Header, Status};
use rocket::response::Responder;
use rocket::shield::{Permission, Policy, Shield};
use rocket::{catch, catchers, get, launch, routes, Build, Config, Request, Response, Rocket};
use serde::Deserialize;
use serde_json::json;
use thiserror::Error;

/// Connecting to a service blocked in China gets silently dropped, so we need a timeout.
/// Around 10 seconds is the max time it takes to handle everything from Shanghai.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(7);

static CLIENT: Lazy<Client> = Lazy::new(|| {
    ClientBuilder::new()
        .timeout(REQUEST_TIMEOUT)
        .insert_resolve_overrides()
        .danger_accept_invalid_hostnames(true) // TODO: Looser than I'd like.
        .build()
        .unwrap()
});

trait ClientBuilderExt {
    fn insert_resolve_overrides(self) -> Self;
}

impl ClientBuilderExt for ClientBuilder {
    /// Resolver overrides with a few IPs hard-coded. Sometimes the Chinese DNS won't resolve
    /// Twitch's domains. It's inconsistent enough that I could *probably* just retry it,
    /// but these IPs have been stable for years so save time and hardcode them.
    ///
    /// Doing this appears to reduce latency variation even when the DNS is working.
    fn insert_resolve_overrides(self) -> Self {
        self.resolve("twitch.map.fastly.net", socket_addr_v4([151, 101, 110, 167], 443))
            .resolve("usher.ttvnw.net", socket_addr_v4([23, 160, 0, 254], 443))
        // the fastly IP hasn't changed in the last three years
        // the ttvnw IP is also at least two years old
        // if they start changing, make it part of the build process
        // note alternative usher IP: [192, 108, 239, 254], 443
    }
}

/// Just to make formatting cleaner.
fn socket_addr_v4(ip: [u8; 4], port: u16) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::from(ip), port))
}

#[launch]
fn rocket() -> Rocket<Build> {
    let config = Config {
        port: get_port(),
        address: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        workers: 4, // we're way below the minimum 128MB RAM, at ~10MB
        keep_alive: 0,
        ..Default::default()
    };
    // use a non-default Shield that only blocks FLoC and adds a CORS header
    // the default also has NoSniff and anti-framejacking stuff that we don't need
    let shield = Shield::new().enable(Permission::default()).enable(LaxCORSOrigin);
    #[cfg(not(feature = "resolve"))]
    let routes = routes![process_live, process_vod];
    #[cfg(feature = "resolve")]
    let routes = routes![process_live, process_vod, resolve];
    rocket::custom(&config).attach(shield).register("/", catchers![not_found]).mount("/", routes)
}

/// CORS header to allow all origins.
#[derive(Copy, Clone, Debug, Default)]
struct LaxCORSOrigin;

impl Policy for LaxCORSOrigin {
    const NAME: &'static str = "Access-Control-Allow-Origin";

    fn header(&self) -> Header<'static> {
        Header::new(Self::NAME, "*")
    }
}

/// Get port from defaults or environment variable.
fn get_port() -> u16 {
    const DEFAULT: &str = if cfg!(feature = "azure") { "8080" } else { "9000" };
    /// This is an Azure env var but can be set in Aliyun if wanted.
    const PORT_KEY: &str = "FUNCTIONS_CUSTOMHANDLER_PORT";
    env::var(PORT_KEY).as_deref().unwrap_or(DEFAULT).parse().expect("port")
}

/// Catch 404 and show what URL was requested.
#[catch(404)]
fn not_found(req: &Request) -> String {
    format!("{} does not exist", req.uri())
}

/// Endpoint to print resolved IPs. Useful when running inside China to find current IPs
/// for CDNs and such things, for hardcoding into HardResolver.
/// Not enabled by default both because it's useless outside of that and for legal reasons.
#[cfg(feature = "resolve")]
#[cfg_attr(feature = "azure", get("/api/resolve/<domain>"))] // XXX missing func definition
#[cfg_attr(feature = "aliyun", get("/2016-08-15/proxy/a/prx/invoke/resolve/<domain>"))]
fn resolve(domain: &str) -> String {
    use std::net::ToSocketAddrs;
    use std::time::Instant;

    let start = Instant::now();
    let addrs = domain.to_socket_addrs().expect("tsa").collect::<Vec<_>>();
    let end = Instant::now();
    json!({
        "time": end.duration_since(start).as_secs_f64(),
        "addrs": addrs,
    })
    .to_string()
}

// XXX It would be nice if the endpoint was configurable somehow due to containing the service/fn name
#[cfg_attr(feature = "azure", get("/api/live/<channel>"))]
#[cfg_attr(feature = "aliyun", get("/2016-08-15/proxy/a/prx/invoke/live/<channel>"))]
async fn process_live(channel: &str) -> Result<M3U8Responder, ErrorResponder> {
    process(Variables::Channel(channel.to_lowercase())).await
}

#[cfg_attr(feature = "azure", get("/api/vod/<id>"))]
#[cfg_attr(feature = "aliyun", get("/2016-08-15/proxy/a/prx/invoke/vod/<id>"))]
async fn process_vod(id: u64) -> Result<M3U8Responder, ErrorResponder> {
    process(Variables::VOD(id.to_string())).await
}

async fn process(var: Variables) -> Result<M3U8Responder, ErrorResponder> {
    let token = get_access_token(&var).await.into_responder("GQL")?.data.playback_access_token;
    let m3u8 = get_m3u8(&var.get_url(), token).await.into_responder("M3U")?;
    Ok(M3U8Responder(m3u8))
}

async fn get_m3u8(url: &str, token: PlaybackAccessToken) -> Result<String, Error> {
    let mut pcg = get_rng();
    let p = pcg.gen_range(0..=9_999_999).to_string();
    CLIENT
        .get(url)
        .query(&token.gen_query(&p, &generate_id().to_lowercase()))
        .send()
        .await?
        .text()
        .await
        .map_err(|e| e.into())
}

trait ResultExt<T> {
    /// Convert the Error in this Result (if present) into an ErrorResponder.
    fn into_responder(self, stage: &'static str) -> Result<T, ErrorResponder>;
}

impl<T> ResultExt<T> for Result<T, Error> {
    fn into_responder(self, stage: &'static str) -> Result<T, ErrorResponder> {
        self.map_err(|e| ErrorResponder(e, stage))
    }
}

#[derive(Clone, Debug)]
pub(crate) struct M3U8Responder(pub(crate) String);

impl<'a> Responder<'a, 'static> for M3U8Responder {
    fn respond_to(self, _: &'a Request<'_>) -> rocket::response::Result<'static> {
        // Aliyun doesn't allow Gzip
        Response::build()
            .header(Header::new("Cache-Control", "no-store"))
            .header(ContentType::new("application", "vnd.apple.mpegurl")) // exact type from twitch
            .sized_body(self.0.len(), io::Cursor::new(self.0))
            .ok()
    }
}

/// Client-ID of Twitch's web player. Shown in the clear if you load the main page.
/// Try `curl -s https://www.twitch.tv | tidy -q | grep '"Client-ID":"'`.
const TWITCH_CLIENT: &str = "kimne78kx3ncx6brgo4mv6wki5h1ko";

/// Asks Twitch for an access token using a randomly-generated ID.
///
/// Could *probably* also skip this step and use your real ID. Faster but less private, which
/// may be a dealbreaker. Might be required server-side if you watch any subscriber-only VODs,
/// but you wouldn't get ads anyway so the extension's fail-safe should prevent it from
/// actually breaking client-side.
async fn get_access_token(var: &Variables) -> Result<AccessTokenResponse, Error> {
    let request = json!({
        "operationName": "PlaybackAccessToken",
        "extensions": {
            "persistedQuery": {
                "version": 1,
                "sha256Hash": "0828119ded1c13477966434e15800ff57ddacf13ba1911c129dc2200705b0712",
            },
        },
        "variables": {
            "isLive": matches!(var, Variables::Channel(_)),
            "login": if matches!(var, Variables::Channel(_)) { var.data() } else { "" },
            "isVod": matches!(var, Variables::VOD(_)),
            "vodID": if matches!(var, Variables::VOD(_)) { var.data() } else { "" },
            "playerType": "site", // "embed" may also be valid
        },
    });
    let id = generate_id();
    // Send a request to fastly (accessible in China)
    // and tell it we want to talk to Twitch's GQL API (blocked in China)
    // This workaround is necessary even with the hard-coded resolver due to TLS SNI
    // sending the hostname in the clear.
    CLIENT
        .post("https://twitch.map.fastly.net/gql")
        .header("Host", "gql.twitch.tv")
        .header("Client-ID", TWITCH_CLIENT)
        .header("Device-ID", &id)
        .json(&request)
        .send()
        .await?
        .json()
        .await
        .map_err(|e| e.into())
}

/// Holds an Error and the stage at which it occurred (GQL token or M3U playlist) and
/// responds in JSON format for programmatic handling.
pub(crate) struct ErrorResponder(Error, &'static str);

impl fmt::Display for ErrorResponder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl fmt::Debug for ErrorResponder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}
impl std::error::Error for ErrorResponder {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.source()
    }
}

impl<'a> Responder<'a, 'a> for ErrorResponder {
    fn respond_to(self, _: &'a Request<'_>) -> rocket::response::Result<'a> {
        // codes are nonsense, just to make it slightly easier to distinguish them
        let code = match &self.0 {
            Error::Http(e) => {
                if e.is_timeout() {
                    504
                } else {
                    e.status().map(|s| s.as_u16()).unwrap_or(510)
                }
            }
            Error::Serde(_) => 501,
        };
        let json = self.0.to_json(self.1).to_string();
        Response::build()
            .status(Status::from_code(code).expect("code"))
            .sized_body(json.len(), io::Cursor::new(json))
            .ok()
    }
}

#[derive(Debug, Error)]
pub(crate) enum Error {
    #[error("http error")]
    Http(#[from] reqwest::Error),
    #[error("serde error")]
    Serde(#[from] serde_json::Error),
}

impl Error {
    pub(crate) fn to_json(&self, stage: &str) -> serde_json::Value {
        json!({
            "result": "error",
            "stage": stage,
            "debug": format!("{:?}", self),
            "display": format!("{}", self),
        })
    }
}

fn get_rng() -> impl Rng {
    Pcg64::from_entropy()
}

/// Generate an ID suitable for use both as a Device-ID and a play_session_id.
/// The latter must be lowercased.
///
/// Both are 32-character alphanumeric strings.
fn generate_id() -> String {
    let mut pcg = get_rng();
    std::iter::repeat(()).map(|_| pcg.sample(Alphanumeric)).map(char::from).take(32).collect()
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct AccessTokenResponse {
    pub(crate) data: Data,
    pub(crate) extensions: Extensions,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Data {
    /// The signed access token itself.
    ///
    /// Can in fact be `null`, for example if the VOD ID is wrong or pointing to a deleted VOD.
    /// Not modeled since we want to error out anyway. TODO: Model it so we can make a nicer error?
    // Name depends on whether it's a livestream or a VOD.
    #[serde(rename = "streamPlaybackAccessToken", alias = "videoPlaybackAccessToken")]
    pub(crate) playback_access_token: PlaybackAccessToken,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct PlaybackAccessToken {
    pub(crate) value: String,
    pub(crate) signature: String,
    #[serde(rename = "__typename")]
    pub(crate) typename: String,
}

impl PlaybackAccessToken {
    fn gen_query<'a>(&'a self, p: &'a str, play_session_id: &'a str) -> [(&str, &str); 12] {
        // XXX should probably send slightly different things for a VOD? it's working so I haven't
        //  bothered to check
        [
            ("player_backend", "mediaplayer"),
            ("playlist_include_framerate", "true"),
            ("reassignments_supported", "true"),
            ("supported_codecs", "vp09,avc1"), // firefox only sends avc1
            ("play_session_id", play_session_id),
            ("cdm", "wv"),
            ("player_version", "1.4.0"),
            ("fast_bread", "true"), // enables low latency for live
            ("token", &self.value),
            ("sig", &self.signature),
            ("allow_source", "true"),
            ("p", &p),
        ]
    }
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Extensions {
    #[serde(rename = "durationMilliseconds")]
    pub(crate) duration_milliseconds: i64,
    #[serde(rename = "operationName")]
    pub(crate) operation_name: String,
    #[serde(rename = "requestID")]
    pub(crate) request_id: String,
}

#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Variables {
    Channel(String),
    VOD(String),
}

impl Variables {
    pub(crate) fn get_url(&self) -> String {
        const BASE: &str = "https://usher.ttvnw.net/";
        let endpoint = match &self {
            Self::Channel(channel) => format!("api/channel/hls/{}.m3u8", channel),
            Self::VOD(id) => format!("vod/{}.m3u8", id),
        };
        format!("{}{}", BASE, endpoint)
    }
    pub(crate) fn data(&self) -> &str {
        match self {
            Self::Channel(d) | Self::VOD(d) => &d,
        }
    }
}
