//! Native Discord local-RPC integration.
//!
//! Replaces the `discord-deckboard` extension, which needs node sockets to
//! reach Discord's named pipe. The protocol (from discord-rpc, mirrored
//! here): connect to `\\?\pipe\discord-ipc-N`, send a handshake frame
//! (opcode 0) with the OAuth2 client id, authenticate with the access
//! token the user saved in the original app, then talk commands (opcode 1)
//! GET/SET_VOICE_SETTINGS and SELECT_VOICE_CHANNEL. Frames are
//! `[i32 LE opcode][i32 LE length][json]`.
//!
//! One authenticated connection is kept alive for the process lifetime
//! ([`DiscordClient`]). Opening a session costs ~400 ms inside Discord
//! (handshake READY + AUTHENTICATE), which is why the earlier
//! session-per-click design lagged; on an established connection an action
//! is a single sub-millisecond round trip. The actor thread answers
//! Discord's pings while idle and keeps a mute/deaf cache fed by
//! VOICE_SETTINGS_UPDATE pushes, so toggles send SET straight away instead
//! of GET-then-SET.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

#[derive(Debug, thiserror::Error)]
pub enum DiscordError {
    #[error("Discord is not running (no discord-ipc pipe found)")]
    NotRunning,
    #[error("Discord rejected the saved access token - re-authorization needed")]
    AuthRejected,
    #[error("authorization popup was declined or timed out")]
    AuthCancelled,
    #[error("network unreachable for {0}")]
    /// Transport-level failure: the request never reached Discord (or the
    /// reply never came back). Callers must distinguish this from a real
    /// rejection - a popup cannot help while the machine is offline.
    Network(&'static str),
    #[error("Discord RPC call failed: {0}")]
    Call(&'static str),
    #[error("bad payload for {0}: {1}")]
    BadPayload(&'static str, String),
}

pub type Result<T> = std::result::Result<T, DiscordError>;

/// Credentials read from `~/pulpitApp/settings.json` (the original app's
/// config fields for the discord-deckboard package).
#[derive(Clone, Default)]
pub struct DiscordConfig {
    pub client_id: String,
    pub client_secret: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for DiscordConfig {
    /// Secrets must never reach logs (kept 14 days): every credential is
    /// redacted, only the public client id prints for correlation.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscordConfig")
            .field("client_id", &self.client_id)
            .field("client_secret", &"<redacted>")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Scopes the original extension requests - voice control needs
/// rpc.voice.read/write.
pub const SCOPES: &[&str] = &[
    "identify",
    "rpc",
    "rpc.notifications.read",
    "rpc.voice.read",
    "rpc.voice.write",
    "rpc.activities.write",
];

/// The redirect URI the original app registers (never actually opened -
/// the code arrives over the local RPC pipe).
const REDIRECT_URI: &str = "https://discord.com";

impl DiscordConfig {
    /// `settings["discord-deckboard"]["<field>"]["value"]` shape.
    pub fn from_settings(settings: &Value) -> Option<DiscordConfig> {
        let package = settings.get("discord-deckboard")?;
        let field = |name: &str| {
            package
                .get(name)
                .and_then(|f| f.get("value"))
                .and_then(Value::as_str)
                .map(str::to_string)
                .filter(|s| !s.is_empty())
        };
        Some(DiscordConfig {
            client_id: field("discordClientId")?,
            client_secret: field("discordClientSecret").unwrap_or_default(),
            access_token: field("discordAccessToken").unwrap_or_default(),
            refresh_token: field("discordRefreshToken"),
        })
    }
}

/// Tokens issued by the OAuth exchange.
pub struct AuthTokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
}

impl std::fmt::Debug for AuthTokens {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AuthTokens")
            .field("access_token", &"<redacted>")
            .field(
                "refresh_token",
                &self.refresh_token.as_ref().map(|_| "<redacted>"),
            )
            .finish()
    }
}

/// Bound on a single HTTP call to Discord (OAuth exchange): a wedged
/// network must not pin the calling action thread indefinitely.
const HTTP_CALL_TIMEOUT: Duration = Duration::from_secs(30);
/// Local port probes answer instantly or refuse the connection; anything
/// still silent after this is not Discord.
const HTTP_PROBE_TIMEOUT: Duration = Duration::from_secs(2);

/// Discord's local HTTP endpoint (the port that answers 404 on /).
fn find_endpoint() -> Result<String> {
    let agent = http_agent(HTTP_PROBE_TIMEOUT);
    for port in 6463..6473 {
        let url = format!("http://127.0.0.1:{port}");
        // ureq reports 404 as an error unless configured otherwise, and
        // Discord's endpoint is exactly the port answering 404 on /
        if let Ok(resp) = agent.get(&url).call() {
            if resp.status().as_u16() == 404 {
                return Ok(url);
            }
        }
    }
    Err(DiscordError::NotRunning)
}

/// HTTP client that returns every status as a Response (Discord's local
/// API answers 404/400 on purpose), with a per-call timeout.
fn http_agent(timeout: Duration) -> ureq::Agent {
    pulpit_db::http_agent(timeout, false)
}

/// x-www-form-urlencoded body from key/value pairs.
fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={}", urlencode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn urlencode(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn post_form(url: &str, body: &str) -> Result<Value> {
    // Transport failures are `Network`, not a rejection: the request never
    // reached Discord, so nothing Discord said can be inferred from it.
    let resp = http_agent(HTTP_CALL_TIMEOUT)
        .post(url)
        .content_type("application/x-www-form-urlencoded")
        .header("Authorization", "Bearer null") // discord-rpc sends this too
        .send(body.as_bytes())
        .map_err(|_| DiscordError::Network("oauth request"))?;
    let text = resp
        .into_body()
        .read_to_string()
        .map_err(|_| DiscordError::Network("oauth body"))?;
    let parsed: Value =
        serde_json::from_str(&text).map_err(|_| DiscordError::Call("oauth json"))?;
    if parsed.get("access_token").is_none() {
        // {"error": "invalid_client", ...} - Discord itself answered
        return Err(DiscordError::Call("oauth error response"));
    }
    Ok(parsed)
}

/// Full interactive authorization: shows Discord's consent popup on the
/// desktop and exchanges the code for tokens. The user must click Allow.
pub fn authorize(config: &DiscordConfig, deadline: Instant) -> Result<AuthTokens> {
    if config.client_id.is_empty() || config.client_secret.is_empty() {
        return Err(DiscordError::BadPayload(
            "config",
            "client id / secret missing in settings".into(),
        ));
    }
    // step 1 (optional): short-lived rpc token. Newer Discord builds
    // dropped the local /oauth2/token routes - a failure here is fine,
    // AUTHORIZE accepts the flow without an rpc token.
    let rpc_token = match find_endpoint() {
        Ok(endpoint) => {
            let body = form(&[
                ("client_id", &config.client_id),
                ("client_secret", &config.client_secret),
            ]);
            post_form(&format!("{endpoint}/oauth2/token/rpc"), &body)
                .ok()
                .and_then(|v| {
                    v.get("rpc_token")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
        }
        Err(_) => None,
    };

    // step 2: AUTHORIZE over the pipe - Discord shows the consent popup
    let mut session = Conn::handshake(&config.client_id, deadline)?;
    let mut args = json!({
        "scopes": SCOPES,
        "client_id": config.client_id,
        "prompt": "consent",
    });
    if let Some(token) = &rpc_token {
        args["rpc_token"] = json!(token);
    }
    let reply = session.request_until(deadline, "AUTHORIZE", args)?;
    if reply.get("evt").and_then(Value::as_str) == Some("ERROR") {
        let msg = reply
            .pointer("/data/message")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        tracing::warn!(message = msg, "discord AUTHORIZE rejected");
        return Err(DiscordError::AuthCancelled);
    }
    let code = reply
        .pointer("/data/code")
        .and_then(Value::as_str)
        .ok_or(DiscordError::AuthCancelled)?
        .to_string();

    // step 3: exchange the code on the real API (local routes are gone in
    // newer Discord builds)
    let body = form(&[
        ("client_id", &config.client_id),
        ("client_secret", &config.client_secret),
        ("code", &code),
        ("grant_type", "authorization_code"),
        ("redirect_uri", REDIRECT_URI),
    ]);
    let token_json = post_form("https://discord.com/api/oauth2/token", &body)?;
    Ok(AuthTokens {
        access_token: token_json
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or(DiscordError::Call("missing access_token"))?
            .to_string(),
        refresh_token: token_json
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

/// Silent token refresh; works while a refresh token from a previous
/// authorization is saved.
pub fn refresh(config: &DiscordConfig) -> Result<AuthTokens> {
    let refresh_token = config
        .refresh_token
        .as_deref()
        .ok_or(DiscordError::AuthRejected)?;
    let body = form(&[
        ("client_id", &config.client_id),
        ("client_secret", &config.client_secret),
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh_token),
        ("redirect_uri", REDIRECT_URI),
    ]);
    let token_json = post_form("https://discord.com/api/oauth2/token", &body)?;
    Ok(AuthTokens {
        access_token: token_json
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or(DiscordError::AuthRejected)?
            .to_string(),
        refresh_token: token_json
            .get("refresh_token")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| config.refresh_token.clone()),
    })
}

/// Persist tokens back into settings.json in the original app's shape
/// (`discord-deckboard.discordAccessToken.value` etc.), leaving every
/// other field untouched.
pub fn save_tokens(path: &std::path::Path, tokens: &AuthTokens) -> std::io::Result<()> {
    // A MISSING file is the only case that starts from `{}`. A file that
    // exists but cannot be read or parsed must error without writing, or
    // every other extension's config would be silently wiped by the
    // merge below.
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(e),
    };
    let mut settings: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return Err(corrupt_settings(e)),
    };
    // A settings.json that parses but is not an object (or whose
    // `discord-deckboard` field is not an object) cannot be merged into.
    // Error instead of panicking: this runs inside a background re-auth
    // task, where a panic would vanish into a swallowed JoinError and the
    // tokens would silently never persist.
    let obj = settings
        .as_object_mut()
        .ok_or_else(|| not_an_object(&raw))?;
    let package = obj
        .entry("discord-deckboard")
        .or_insert_with(|| Value::Object(Default::default()));
    let package = package.as_object_mut().ok_or_else(|| not_an_object(&raw))?;
    let mut field = |name: &str, value: &str| {
        let entry = package.entry(name.to_string()).or_insert_with(|| {
            json!({
                "descriptions": "Discord OAuth token (managed by pulpit-server)",
                "name": name,
                "type": "text",
                "value": "",
            })
        });
        if let Some(f) = entry.as_object_mut() {
            f.insert("value".into(), json!(value));
        }
    };
    field("discordAccessToken", &tokens.access_token);
    if let Some(r) = &tokens.refresh_token {
        field("discordRefreshToken", r);
    }
    // The parsed value is re-serialized, so this cannot realistically
    // fail - but a silent empty-file write must never happen either.
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|e| std::io::Error::other(format!("settings serialization failed: {e}")))?;
    pulpit_db::write_atomic(path, json.as_bytes())
}

/// Persist client_id (and, when given, client_secret) into settings.json
/// under `discord-deckboard`, preserving every other field and extension
/// configuration. `None` keeps the stored secret - the settings UI never
/// echoes it, so an empty secret input means "unchanged", not "erase".
pub fn save_config(
    path: &std::path::Path,
    client_id: &str,
    client_secret: Option<&str>,
) -> std::io::Result<()> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => "{}".into(),
        Err(e) => return Err(e),
    };
    let mut settings: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return Err(corrupt_settings(e)),
    };
    let obj = settings
        .as_object_mut()
        .ok_or_else(|| not_an_object(&raw))?;
    let package = obj
        .entry("discord-deckboard")
        .or_insert_with(|| Value::Object(Default::default()));
    let package = package.as_object_mut().ok_or_else(|| not_an_object(&raw))?;
    let mut field = |name: &str, value: &str, desc: &str, ftype: &str| {
        let entry = package.entry(name.to_string()).or_insert_with(|| {
            json!({
                "descriptions": desc,
                "name": name,
                "type": ftype,
                "value": "",
            })
        });
        if let Some(f) = entry.as_object_mut() {
            f.insert("value".into(), json!(value));
        }
    };
    field(
        "discordClientId",
        client_id.trim(),
        "Discord Application Client ID",
        "text",
    );
    if let Some(secret) = client_secret {
        field(
            "discordClientSecret",
            secret.trim(),
            "Discord Application Client Secret",
            "password",
        );
    }

    let json = serde_json::to_string_pretty(&settings)
        .map_err(|e| std::io::Error::other(format!("settings serialization failed: {e}")))?;
    pulpit_db::write_atomic(path, json.as_bytes())
}

/// Clear saved Discord OAuth tokens from settings.json while keeping client_id,
/// client_secret, and all other extensions intact.
pub fn clear_tokens(path: &std::path::Path) -> std::io::Result<()> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut settings: Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return Err(corrupt_settings(e)),
    };
    let Some(obj) = settings.as_object_mut() else {
        return Ok(());
    };
    if let Some(package) = obj
        .get_mut("discord-deckboard")
        .and_then(Value::as_object_mut)
    {
        if let Some(f) = package
            .get_mut("discordAccessToken")
            .and_then(Value::as_object_mut)
        {
            f.insert("value".into(), json!(""));
        }
        if let Some(f) = package
            .get_mut("discordRefreshToken")
            .and_then(Value::as_object_mut)
        {
            f.insert("value".into(), json!(""));
        }
    }
    let json = serde_json::to_string_pretty(&settings)
        .map_err(|e| std::io::Error::other(format!("settings serialization failed: {e}")))?;
    pulpit_db::write_atomic(path, json.as_bytes())
}

/// Status of the Discord RPC connection.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DiscordStatus {
    NotConfigured,
    NotRunning,
    NeedsAuth,
    Connected { username: String },
    Error { message: String },
}

/// Probe Discord connection status.
pub fn probe_status(config: &DiscordConfig) -> DiscordStatus {
    if config.client_id.trim().is_empty() {
        return DiscordStatus::NotConfigured;
    }
    if config.access_token.trim().is_empty() {
        return DiscordStatus::NeedsAuth;
    }
    let deadline = Instant::now() + Duration::from_millis(1500);
    match Conn::connect(config, deadline) {
        Ok(conn) => {
            let user = conn.username.unwrap_or_else(|| "Discord".to_string());
            DiscordStatus::Connected { username: user }
        }
        Err(DiscordError::NotRunning) => DiscordStatus::NotRunning,
        Err(DiscordError::AuthRejected) => match refresh(config) {
            Ok(new_tokens) => {
                let mut fresh = config.clone();
                fresh.access_token = new_tokens.access_token;
                fresh.refresh_token = new_tokens.refresh_token;
                let retry_deadline = Instant::now() + Duration::from_millis(1500);
                match Conn::connect(&fresh, retry_deadline) {
                    Ok(conn) => {
                        let user = conn.username.unwrap_or_else(|| "Discord".to_string());
                        DiscordStatus::Connected { username: user }
                    }
                    Err(DiscordError::NotRunning) => DiscordStatus::NotRunning,
                    Err(_) => DiscordStatus::NeedsAuth,
                }
            }
            Err(_) => DiscordStatus::NeedsAuth,
        },
        Err(e) => DiscordStatus::Error {
            message: e.to_string(),
        },
    }
}

/// Parse failure of an existing settings.json. The serde message carries
/// only positions, never file content - the file may hold credentials.
fn corrupt_settings(e: serde_json::Error) -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("settings.json exists but is not valid JSON: {e}"),
    )
}

fn not_an_object(raw: &str) -> std::io::Error {
    let excerpt: String = raw.chars().take(40).collect();
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("settings.json is not a JSON object: {excerpt}"),
    )
}

/// Is this action one of ours?
pub fn is_discord_action(kind: &str) -> bool {
    matches!(
        kind,
        "toggle-microphone"
            | "toggle-headphone"
            | "microphone"
            | "headphone"
            | "disconnect-voice"
            | "connect-voice"
            | "change-input"
    )
}

/// Input declarations for the style resolver: (value, icon, color, mode).
pub fn input_declarations() -> Vec<(
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
)> {
    vec![
        (
            "toggle-microphone",
            "microphone",
            "#5865F2",
            Some("custom-value"),
        ),
        (
            "toggle-headphone",
            "headphones",
            "#5865F2",
            Some("custom-value"),
        ),
        ("microphone", "microphone", "#5865F2", None),
        ("headphone", "headphones", "#5865F2", None),
        ("disconnect-voice", "phone-slash", "#5865F2", None),
        ("connect-voice", "phone", "#5865F2", None),
        ("change-input", "wave-square", "#5865F2", None),
    ]
}

// ------------------------------------------------------------------ frames

const OP_HANDSHAKE: u32 = 0;
const OP_FRAME: u32 = 1;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

/// Largest inbound frame the client will ever assemble; real Discord
/// frames are a few KB of JSON. A length header beyond this means desync
/// or a hostile peer - the receive buffer is dropped instead of growing
/// toward the claimed size (up to 2 GiB).
const MAX_FRAME_LEN: usize = 1 << 20; // 1 MiB

pub fn encode_frame(op: u32, payload: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 + payload.len());
    buf.extend_from_slice(&(op as i32).to_le_bytes());
    buf.extend_from_slice(&(payload.len() as i32).to_le_bytes());
    buf.extend_from_slice(payload.as_bytes());
    buf
}

/// Payload length claimed by the frame header at the front of `buf`.
fn frame_len(buf: &[u8]) -> usize {
    let bytes: [u8; 4] = buf
        .get(4..8)
        .and_then(|s| s.try_into().ok())
        .unwrap_or([0; 4]);
    i32::from_le_bytes(bytes).max(0) as usize
}

/// Parse `[op][len][payload]` from a buffer; None while incomplete. A
/// claimed length beyond [`MAX_FRAME_LEN`] also yields None - the caller
/// drops such buffers instead of waiting the claimed bytes out.
pub fn decode_frame(buf: &[u8]) -> Option<(u32, &[u8])> {
    if buf.len() < 8 {
        return None;
    }
    let op = u32::from_le_bytes(buf[0..4].try_into().ok()?) as i32;
    let len = frame_len(buf);
    if len > MAX_FRAME_LEN {
        return None;
    }
    if buf.len() < 8 + len {
        return None;
    }
    Some((op as u32, &buf[8..8 + len]))
}

/// Pop every complete frame from the front of `buf`, parsed as JSON. An
/// incomplete tail stays put; a frame whose payload is not JSON is dropped
/// (it cannot participate in request/reply matching anyway).
fn extract_frames(buf: &mut Vec<u8>) -> Vec<(u32, Value)> {
    let mut frames = Vec::new();
    while buf.len() >= 8 {
        if frame_len(buf) > MAX_FRAME_LEN {
            tracing::warn!(
                claimed = frame_len(buf),
                cap = MAX_FRAME_LEN,
                "discord frame length beyond cap, dropping the receive buffer"
            );
            buf.clear();
            break;
        }
        let Some((op, payload)) = decode_frame(buf) else {
            break;
        };
        let payload = payload.to_vec();
        buf.drain(..8 + payload.len());
        match serde_json::from_slice(&payload) {
            Ok(v) => frames.push((op, v)),
            Err(_) => tracing::debug!(bytes = payload.len(), "dropping non-json discord frame"),
        }
    }
    frames
}

/// What one inbound frame asks of the connection.
#[derive(Debug, PartialEq)]
enum Incoming {
    /// Nonce-matched reply to a request we sent (successful or not - the
    /// caller inspects `evt`).
    Reply,
    /// Discord pushed the current voice settings (mute/deaf changed,
    /// including changes made inside Discord itself).
    VoiceSettings(Value),
    /// Keepalive ping; answered by echoing the payload back as a pong.
    Ping(Value),
    /// READY, activity dispatches, frames for other nonces: ignore.
    Other,
}

/// `nonce` is Some while waiting for a specific request's reply, None
/// while idle (nothing we sent is outstanding).
fn classify(op: u32, frame: &Value, nonce: Option<&str>) -> Incoming {
    if op == OP_PING {
        return Incoming::Ping(frame.clone());
    }
    if nonce.is_some() && frame.get("nonce").and_then(Value::as_str) == nonce {
        return Incoming::Reply;
    }
    if frame["cmd"].as_str() == Some("DISPATCH")
        && frame["evt"].as_str() == Some("VOICE_SETTINGS_UPDATE")
    {
        return Incoming::VoiceSettings(frame["data"].clone());
    }
    Incoming::Other
}

// ------------------------------------------------------------- pipe client

/// Second wait slot in [`Pipe::read_some_wake`]: the wake event (the
/// first slot, the read event, answers as plain WAIT_OBJECT_0).
const WAIT_WAKE: u32 = pipe::WAIT_OBJECT_0 + 1;

#[cfg(windows)]
mod pipe {
    use std::ffi::c_void;

    pub const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;
    pub const GENERIC_READ_WRITE: u32 = 0x8000_0000 | 0x4000_0000;
    pub const OPEN_EXISTING: u32 = 3;
    pub const FILE_FLAG_OVERLAPPED: u32 = 0x4000_0000;
    pub const ERROR_IO_PENDING: u32 = 997;
    pub const WAIT_OBJECT_0: u32 = 0;
    pub const WAIT_TIMEOUT: u32 = 258;

    /// Layout of the Win32 OVERLAPPED structure.
    #[repr(C)]
    pub struct Overlapped {
        pub internal: usize,
        pub internal_high: usize,
        pub offset: u32,
        pub offset_high: u32,
        pub event: *mut c_void,
    }

    impl Overlapped {
        pub fn zeroed(event: *mut c_void) -> Self {
            Overlapped {
                internal: 0,
                internal_high: 0,
                offset: 0,
                offset_high: 0,
                event,
            }
        }
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn CreateFileW(
            name: *const u16,
            access: u32,
            share: u32,
            security: *mut c_void,
            disposition: u32,
            flags: u32,
            template: *mut c_void,
        ) -> *mut c_void;
        pub fn ReadFile(
            handle: *mut c_void,
            buffer: *mut u8,
            to_read: u32,
            read: *mut u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        pub fn WriteFile(
            handle: *mut c_void,
            buffer: *const u8,
            to_write: u32,
            written: *mut u32,
            overlapped: *mut Overlapped,
        ) -> i32;
        pub fn GetOverlappedResult(
            handle: *mut c_void,
            overlapped: *mut Overlapped,
            bytes: *mut u32,
            wait: i32,
        ) -> i32;
        pub fn CancelIoEx(handle: *mut c_void, overlapped: *mut Overlapped) -> i32;
        pub fn CreateEventW(
            security: *mut c_void,
            manual_reset: i32,
            initial_state: i32,
            name: *const u16,
        ) -> *mut c_void;
        pub fn CloseHandle(handle: *mut c_void) -> i32;
        pub fn SetEvent(handle: *mut c_void) -> i32;
        pub fn WaitForSingleObject(handle: *mut c_void, milliseconds: u32) -> u32;
        pub fn WaitForMultipleObjects(
            count: u32,
            handles: *const *mut c_void,
            wait_all: i32,
            milliseconds: u32,
        ) -> u32;
        pub fn GetLastError() -> u32;

        // test-only server side of a fake Discord pipe
        #[cfg(test)]
        pub fn CreateNamedPipeW(
            name: *const u16,
            open_mode: u32,
            pipe_mode: u32,
            instances: u32,
            out_size: u32,
            in_size: u32,
            default_timeout: u32,
            security: *mut c_void,
        ) -> *mut c_void;
        #[cfg(test)]
        pub fn ConnectNamedPipe(handle: *mut c_void, overlapped: *mut c_void) -> i32;
    }
}

/// A named-pipe handle opened for overlapped I/O, so reads can carry a
/// timeout without a PeekNamedPipe poll loop (the poll's 20 ms sleep was
/// the floor under every request/response).
struct Pipe {
    handle: *mut c_void,
}

// SAFETY: the raw handle is owned exclusively by this struct and every
// method takes &mut self, so calls never overlap.
unsafe impl Send for Pipe {}

/// Auto-reset, unnamed event: each wait consumes one completion. Created
/// per I/O call because an event handed to a completed-synchronously
/// operation stays signaled and would short-circuit the next wait.
fn new_event() -> *mut c_void {
    unsafe { pipe::CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null_mut()) }
}

impl Pipe {
    fn open() -> Result<Pipe> {
        for id in 0..10 {
            let path: Vec<u16> = format!("\\\\?\\pipe\\discord-ipc-{id}\0")
                .encode_utf16()
                .collect();
            // SAFETY: path is NUL-terminated and lives across the call
            let handle = unsafe {
                pipe::CreateFileW(
                    path.as_ptr(),
                    pipe::GENERIC_READ_WRITE,
                    0,
                    std::ptr::null_mut(),
                    pipe::OPEN_EXISTING,
                    pipe::FILE_FLAG_OVERLAPPED,
                    std::ptr::null_mut(),
                )
            };
            if handle != pipe::INVALID_HANDLE_VALUE && !handle.is_null() {
                return Ok(Pipe { handle });
            }
        }
        Err(DiscordError::NotRunning)
    }

    fn write_all(&mut self, mut data: &[u8]) -> Result<()> {
        while !data.is_empty() {
            let written = self.write_once(data)?;
            if written == 0 {
                return Err(DiscordError::Call("WriteFile"));
            }
            data = &data[written..];
        }
        Ok(())
    }

    fn write_once(&self, data: &[u8]) -> Result<usize> {
        let event = new_event();
        if event.is_null() {
            return Err(DiscordError::Call("CreateEventW"));
        }
        let mut overlapped = pipe::Overlapped::zeroed(event);
        let mut written = 0u32;
        // SAFETY: buffer outlives the call, overlapped owns the event
        let ok = unsafe {
            pipe::WriteFile(
                self.handle,
                data.as_ptr(),
                data.len().min(u32::MAX as usize) as u32,
                std::ptr::null_mut(),
                &mut overlapped,
            )
        };
        if ok == 0 {
            let err = unsafe { pipe::GetLastError() };
            if err != pipe::ERROR_IO_PENDING {
                unsafe { pipe::CloseHandle(event) };
                return Err(DiscordError::Call("WriteFile"));
            }
            let wait =
                unsafe { pipe::WaitForSingleObject(event, WRITE_TIMEOUT.as_millis() as u32) };
            if wait != pipe::WAIT_OBJECT_0 {
                // wedged Discord must not hang the actor forever: abandon
                // the write and let the connection die
                unsafe {
                    pipe::CancelIoEx(self.handle, &mut overlapped);
                    pipe::GetOverlappedResult(self.handle, &mut overlapped, &mut written, 1);
                    pipe::CloseHandle(event);
                }
                return Err(DiscordError::Call("WriteFile timeout"));
            }
        }
        let ok =
            unsafe { pipe::GetOverlappedResult(self.handle, &mut overlapped, &mut written, 1) };
        unsafe { pipe::CloseHandle(event) };
        if ok == 0 {
            return Err(DiscordError::Call("GetOverlappedResult(write)"));
        }
        Ok(written as usize)
    }

    /// Wait up to `timeout` for the next bytes and append them to `buf`,
    /// also watching the `wake` event when it is not null. Ok(0) (or
    /// Ok(n, true) for a wake) means nothing arrived in time (the read was
    /// cancelled and the pipe stays usable); Err means the pipe is gone.
    /// The bool result says the wake fired: the read was abandoned so the
    /// caller can re-check the job queue immediately. Any bytes that
    /// arrived alongside the wake are still appended first.
    fn read_some_wake(
        &mut self,
        buf: &mut Vec<u8>,
        timeout: Duration,
        wake: *mut c_void,
    ) -> Result<(usize, bool)> {
        let event = new_event();
        if event.is_null() {
            return Err(DiscordError::Call("CreateEventW"));
        }
        let mut overlapped = pipe::Overlapped::zeroed(event);
        let mut chunk = [0u8; 4096];
        let mut read = 0u32;
        // SAFETY: chunk is a valid receive buffer, overlapped owns the event
        let ok = unsafe {
            pipe::ReadFile(
                self.handle,
                chunk.as_mut_ptr(),
                chunk.len() as u32,
                std::ptr::null_mut(),
                &mut overlapped,
            )
        };
        if ok == 0 {
            let err = unsafe { pipe::GetLastError() };
            if err != pipe::ERROR_IO_PENDING {
                unsafe { pipe::CloseHandle(event) };
                return Err(DiscordError::Call("ReadFile"));
            }
            let wait_ms = timeout.as_millis().min(u32::MAX as u128) as u32;
            // With a wake event, wait on [read event, wake event]; a wake
            // is reported as a timeout plus the `woke` flag so it takes
            // the cancel branch below (the read is still pending and must
            // be abandoned), and the caller re-checks its job queue.
            let (wait, woke) = if wake.is_null() {
                (unsafe { pipe::WaitForSingleObject(event, wait_ms) }, false)
            } else {
                let handles = [event, wake];
                let w = unsafe { pipe::WaitForMultipleObjects(2, handles.as_ptr(), 0, wait_ms) };
                if w == WAIT_WAKE {
                    (pipe::WAIT_TIMEOUT, true)
                } else {
                    (w, false)
                }
            };
            if wait != pipe::WAIT_OBJECT_0 {
                // Timeout or wake: cancel so the pipe is not stuck with a
                // pending read. Any other wait result also leaves the read
                // posted, and the kernel must be told to abandon it before
                // `chunk` goes out of scope - either way drain the aborted
                // call.
                unsafe {
                    pipe::CancelIoEx(self.handle, &mut overlapped);
                    pipe::GetOverlappedResult(self.handle, &mut overlapped, &mut read, 1);
                    pipe::CloseHandle(event);
                }
                if wait == pipe::WAIT_TIMEOUT || woke {
                    if read > 0 {
                        // the operation finished with data right as we
                        // cancelled
                        buf.extend_from_slice(&chunk[..read as usize]);
                        return Ok((read as usize, woke));
                    }
                    return Ok((0, woke));
                }
                return Err(DiscordError::Call("WaitForSingleObject"));
            }
        }
        let ok = unsafe { pipe::GetOverlappedResult(self.handle, &mut overlapped, &mut read, 1) };
        unsafe { pipe::CloseHandle(event) };
        if ok == 0 || read == 0 {
            return Err(DiscordError::Call("GetOverlappedResult(read)"));
        }
        buf.extend_from_slice(&chunk[..read as usize]);
        Ok((read as usize, false))
    }
}

/// Cap on a single pipe write; a healthy local pipe drains instantly, so
/// hitting this means Discord is wedged and the session is not worth saving.
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

impl Drop for Pipe {
    fn drop(&mut self) {
        // SAFETY: handle is a live handle we own
        unsafe { pipe::CloseHandle(self.handle) };
    }
}

// ------------------------------------------------------------- connection

/// One authenticated Discord RPC connection: the pipe plus the frame
/// assembly state and the last known voice settings. Not shared across
/// threads - the actor owns it.
struct Conn {
    pipe: Pipe,
    nonce: u64,
    buf: Vec<u8>,
    inbox: VecDeque<(u32, Value)>,
    /// Last known voice settings, kept fresh by VOICE_SETTINGS_UPDATE
    /// pushes so toggles can send SET without a GET round trip.
    cache: Option<Value>,
    username: Option<String>,
}

impl Conn {
    /// Open the pipe, send the handshake and consume the READY dispatch -
    /// Discord ignores commands sent before READY.
    fn handshake(client_id: &str, deadline: Instant) -> Result<Conn> {
        let mut conn = Conn {
            pipe: Pipe::open()?,
            nonce: 0,
            buf: Vec::new(),
            inbox: VecDeque::new(),
            cache: None,
            username: None,
        };
        conn.pipe.write_all(&encode_frame(
            OP_HANDSHAKE,
            &json!({ "v": 1, "client_id": client_id }).to_string(),
        ))?;
        conn.wait_for_ready(deadline)?;
        Ok(conn)
    }

    /// Handshake + authenticate. Returns the connection with its cache
    /// seeded from Discord's current voice settings.
    fn connect(config: &DiscordConfig, deadline: Instant) -> Result<Conn> {
        if config.client_id.is_empty() || config.client_secret.is_empty() {
            // no OAuth app configured - nothing to authenticate with
            return Err(DiscordError::BadPayload(
                "config",
                "client id / secret missing in settings".into(),
            ));
        }
        if config.access_token.is_empty() {
            // configured but never (re)authorized - the popup flow applies
            return Err(DiscordError::AuthRejected);
        }
        let mut conn = Conn::handshake(&config.client_id, deadline)?;
        let reply = conn.request_until(
            deadline,
            "AUTHENTICATE",
            json!({ "access_token": config.access_token }),
        )?;
        if reply.get("evt").and_then(Value::as_str) == Some("ERROR") {
            return Err(DiscordError::AuthRejected);
        }
        let user = reply
            .pointer("/data/user/global_name")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| reply.pointer("/data/user/username").and_then(Value::as_str))
            .map(str::to_string);
        conn.username = user;
        let reply = conn.request_until(deadline, "GET_VOICE_SETTINGS", json!({}))?;
        require_ok(&reply)?;
        if let Some(data) = reply.get("data").filter(|v| v.is_object()) {
            conn.cache = Some(data.clone());
        }
        Ok(conn)
    }

    fn wait_for_ready(&mut self, deadline: Instant) -> Result<()> {
        let Some((op, frame)) = self.wait_frame(
            deadline,
            Duration::from_secs(1),
            WakeHandle(std::ptr::null_mut()),
        )?
        else {
            return Err(DiscordError::Call("timeout"));
        };
        let is_ready = op == OP_FRAME
            && frame["cmd"].as_str() == Some("DISPATCH")
            && frame["evt"].as_str() == Some("READY");
        if is_ready {
            Ok(())
        } else {
            Err(DiscordError::Call("expected READY dispatch"))
        }
    }

    fn next_nonce(&mut self) -> String {
        self.nonce += 1;
        format!("dk-{}", self.nonce)
    }

    /// Read frames until one complete frame is available or the deadline
    /// passes. Ok(None) on deadline (or on `wake` firing - the caller
    /// re-checks its queue before the deadline matters); single reads
    /// never block longer than `cap`, so an already-passed deadline is
    /// noticed promptly.
    fn wait_frame(
        &mut self,
        deadline: Instant,
        cap: Duration,
        wake: WakeHandle,
    ) -> Result<Option<(u32, Value)>> {
        loop {
            if let Some(frame) = self.inbox.pop_front() {
                return Ok(Some(frame));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            let (read, woke) =
                self.pipe
                    .read_some_wake(&mut self.buf, cap.min(deadline - now), wake.0)?;
            if read > 0 {
                self.inbox.extend(extract_frames(&mut self.buf));
            }
            if woke {
                // a job is queued: let the caller see it now instead of
                // serving the rest of the tick
                return Ok(None);
            }
        }
    }

    /// Answer sideband traffic (pings, voice-settings pushes); Ok(Some)
    /// returns the nonce-matched reply frame the caller is waiting for.
    fn absorb(&mut self, op: u32, frame: Value, nonce: Option<&str>) -> Result<Option<Value>> {
        match classify(op, &frame, nonce) {
            Incoming::Reply => Ok(Some(frame)),
            Incoming::Ping(v) => {
                self.pipe
                    .write_all(&encode_frame(OP_PONG, &v.to_string()))?;
                Ok(None)
            }
            Incoming::VoiceSettings(v) => {
                self.cache = Some(v);
                Ok(None)
            }
            Incoming::Other => Ok(None),
        }
    }

    /// Send a command and wait for its nonce-matched reply (the full
    /// frame - callers check `evt` and read `data` themselves).
    fn request_until(&mut self, deadline: Instant, cmd: &str, args: Value) -> Result<Value> {
        let nonce = self.next_nonce();
        let frame = json!({ "cmd": cmd, "args": args, "nonce": nonce });
        self.pipe
            .write_all(&encode_frame(OP_FRAME, &frame.to_string()))?;
        loop {
            let Some((op, frame)) = self.wait_frame(
                deadline,
                Duration::from_secs(1),
                WakeHandle(std::ptr::null_mut()),
            )?
            else {
                return Err(DiscordError::Call("timeout"));
            };
            if let Some(reply) = self.absorb(op, frame, Some(&nonce))? {
                return Ok(reply);
            }
        }
    }

    /// Answer pings and absorb voice-settings pushes while no action is
    /// running. Returns on the first transport error, when `tick`
    /// elapses, or as soon as `wake` fires - the actor passes the wake
    /// event `execute()` signals, so a queued action ends the idle serve
    /// immediately no matter how long the tick is.
    fn serve(&mut self, tick: Duration, wake: WakeHandle) -> Result<()> {
        let deadline = Instant::now() + tick;
        loop {
            let Some((op, frame)) = self.wait_frame(deadline, tick, wake)? else {
                return Ok(());
            };
            self.absorb(op, frame, None)?;
        }
    }
}

/// Command replies must not be errors; error frames carry their payload in
/// `data` too, and caching or reading mute/deaf out of one would silently
/// fake a successful action.
fn require_ok(frame: &Value) -> Result<()> {
    if frame.get("evt").and_then(Value::as_str) == Some("ERROR") {
        Err(DiscordError::Call("rpc error"))
    } else {
        Ok(())
    }
}

// ------------------------------------------------------------- action plan

/// The original's `_labelMuteDeaf`: muted/deaf show "OFF", live shows "ON".
fn label(active: bool) -> &'static str {
    if active {
        "OFF"
    } else {
        "ON"
    }
}

/// What one action should do. Split from execution so tests can cover the
/// decision logic without touching Discord.
#[derive(Debug, PartialEq)]
pub enum Plan {
    SetMic(bool),
    SetDeaf(bool),
    FlipMic,
    FlipDeaf,
    ConnectChannel(String),
    Disconnect,
    SetInputMode(String),
}

pub fn plan(action: &str, args: &Value) -> Result<Plan> {
    let arg = args.get("action").and_then(Value::as_str).unwrap_or("");
    match action {
        "toggle-microphone" => Ok(Plan::FlipMic),
        "toggle-headphone" => Ok(Plan::FlipDeaf),
        "microphone" => match arg {
            "enable_microphone" => Ok(Plan::SetMic(false)),
            "disable_microphone" => Ok(Plan::SetMic(true)),
            "toggle_microphone" => Ok(Plan::FlipMic),
            other => Err(DiscordError::BadPayload("action", other.into())),
        },
        "headphone" => match arg {
            "enable_headphone" => Ok(Plan::SetDeaf(false)),
            "disable_headphone" => Ok(Plan::SetDeaf(true)),
            "toggle_headphone" => Ok(Plan::FlipDeaf),
            other => Err(DiscordError::BadPayload("action", other.into())),
        },
        "connect-voice" => {
            let channel = args
                .get("channel_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            if channel.is_empty() {
                Ok(Plan::Disconnect)
            } else {
                Ok(Plan::ConnectChannel(channel.to_string()))
            }
        }
        "disconnect-voice" => Ok(Plan::Disconnect),
        "change-input" => match arg {
            "toggle_input" => Ok(Plan::SetInputMode("toggle".into())),
            "push_to_talk" => Ok(Plan::SetInputMode("PUSH_TO_TALK".into())),
            "voice_activity" => Ok(Plan::SetInputMode("VOICE_ACTIVITY".into())),
            other => Err(DiscordError::BadPayload("action", other.into())),
        },
        other => Err(DiscordError::BadPayload("action", other.into())),
    }
}

/// Outcome of a flip: the new state plus the label the original pushed to
/// the custom-value tile.
pub struct FlipOutcome {
    pub label: &'static str,
    pub patch: Value,
}

pub fn apply_flip(current: &Value, what: &Plan) -> Result<Option<FlipOutcome>> {
    let get = |key: &str| {
        current
            .get(key)
            .and_then(Value::as_bool)
            .ok_or(DiscordError::Call("GET_VOICE_SETTINGS"))
    };
    Ok(match what {
        Plan::SetMic(mute) => Some(FlipOutcome {
            label: label(*mute),
            patch: json!({ "mute": mute }),
        }),
        Plan::SetDeaf(deaf) => Some(FlipOutcome {
            label: label(*deaf),
            patch: json!({ "deaf": deaf }),
        }),
        Plan::FlipMic => {
            let now = get("mute")?;
            Some(FlipOutcome {
                label: label(!now),
                patch: json!({ "mute": !now }),
            })
        }
        Plan::FlipDeaf => {
            let now = get("deaf")?;
            Some(FlipOutcome {
                label: label(!now),
                patch: json!({ "deaf": !now }),
            })
        }
        Plan::SetInputMode(mode) => {
            let next = if mode == "toggle" {
                let t = current
                    .pointer("/mode/type")
                    .and_then(Value::as_str)
                    .unwrap_or("VOICE_ACTIVITY");
                if t == "PUSH_TO_TALK" {
                    "VOICE_ACTIVITY"
                } else {
                    "PUSH_TO_TALK"
                }
            } else {
                mode.as_str()
            };
            Some(FlipOutcome {
                label: "",
                patch: json!({ "mode": { "type": next } }),
            })
        }
        Plan::ConnectChannel(_) | Plan::Disconnect => None,
    })
}

// ------------------------------------------------------------ kept-alive client

/// What a finished action wants pushed to tiles: the custom-value key and
/// the state label (same shape as the original's `_labelMuteDeaf` push).
#[derive(Debug)]
pub struct ExecOutcome {
    pub key: String,
    pub label: String,
}

enum Job {
    Exec {
        config: DiscordConfig,
        action: String,
        args: Value,
        deadline: Instant,
        reply: std::sync::mpsc::SyncSender<Result<Option<ExecOutcome>>>,
    },
    Close,
}

/// Handle to the actor thread that owns the Discord connection. The
/// backend shares one instance behind a mutex, so actions queue up and
/// run one at a time.
pub struct DiscordClient {
    jobs: std::sync::Mutex<std::sync::mpsc::Sender<Job>>,
    /// Set after every queued job so the actor's idle pipe read ends at
    /// once instead of at the tick. Shared with the actor thread, so the
    /// handle closes only after both are done with it.
    wake: std::sync::Arc<WakeEvent>,
}

/// Non-owning copy of the wake handle for the serve path; the actor keeps
/// the owning [`WakeEvent`] alive for as long as it uses this copy.
#[derive(Clone, Copy)]
struct WakeHandle(*mut c_void);

unsafe impl Send for WakeHandle {}

/// Auto-reset event the API threads signal to interrupt the actor's idle
/// pipe read ([`Conn::serve`] waits on it next to the read event). The
/// [`DiscordClient`] and the actor thread share it through an `Arc`, so
/// dropping the client never closes a handle the actor is still waiting
/// on (a closed handle value can be reused by an unrelated object).
struct WakeEvent(*mut c_void);

// SAFETY: the raw handle is only ever passed to SetEvent (any thread,
// thread-safe on Windows) and WaitForMultipleObjects (actor thread); it
// carries no interior state, and CloseHandle runs once, on the last drop.
unsafe impl Send for WakeEvent {}
unsafe impl Sync for WakeEvent {}

impl WakeEvent {
    fn new() -> WakeEvent {
        // auto-reset: each wait consumes one signal, so a wake fires
        // exactly one serve interruption
        WakeEvent(unsafe { pipe::CreateEventW(std::ptr::null_mut(), 0, 0, std::ptr::null_mut()) })
    }

    fn handle(&self) -> WakeHandle {
        WakeHandle(self.0)
    }

    fn set(&self) {
        if !self.0.is_null() {
            unsafe { pipe::SetEvent(self.0) };
        }
    }
}

impl Drop for WakeEvent {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { pipe::CloseHandle(self.0) };
        }
    }
}

/// Idle cadence of the actor loop while a connection is up. The tick only
/// bounds how long one serve pass may hold the actor; queued actions
/// interrupt it through the wake event, and with no connection the actor
/// blocks on the job queue outright - so neither state burns wakeups.
const IDLE_TICK: Duration = Duration::from_secs(10);

impl DiscordClient {
    /// Spawn the keep-alive actor. The first action pays the ~400 ms
    /// session setup; every later one is a single round trip.
    pub fn spawn() -> DiscordClient {
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = std::sync::Arc::new(WakeEvent::new());
        let thread_wake = wake.clone();
        std::thread::Builder::new()
            .name("discord-rpc".into())
            .spawn(move || actor_loop(rx, thread_wake, None))
            .expect("spawn discord rpc actor");
        DiscordClient {
            jobs: std::sync::Mutex::new(tx),
            wake,
        }
    }

    /// Execute one action end to end on the shared connection.
    pub fn execute(
        &self,
        config: &DiscordConfig,
        action: &str,
        args: &Value,
        deadline: Instant,
    ) -> Result<Option<ExecOutcome>> {
        let (reply_tx, reply_rx) = std::sync::mpsc::sync_channel(1);
        let job = Job::Exec {
            config: config.clone(),
            action: action.to_string(),
            args: args.clone(),
            deadline,
            reply: reply_tx,
        };
        self.jobs
            .lock()
            .unwrap()
            .send(job)
            .map_err(|_| DiscordError::Call("discord actor stopped"))?;
        // interrupt the actor's idle serve so the job is seen now, not
        // when the tick elapses
        self.wake.set();
        reply_rx
            .recv()
            .map_err(|_| DiscordError::Call("discord actor stopped"))?
    }
}

impl Drop for DiscordClient {
    fn drop(&mut self) {
        // Best effort: if the actor already died the send fails and there
        // is nothing left to stop. The wake ends a running serve early so
        // the Close job is noticed without waiting out the tick.
        let _ = self.jobs.lock().unwrap().send(Job::Close);
        self.wake.set();
    }
}

/// One queued action, or None if the actor should stop. The reply is
/// always sent back (the API thread blocks on it).
fn handle_job(conn: &mut Option<Conn>, job: Job) -> bool {
    match job {
        Job::Close => false,
        Job::Exec {
            config,
            action,
            args,
            deadline,
            reply,
        } => {
            let outcome = run_exec(conn, &config, &action, &args, deadline);
            let _ = reply.send(outcome);
            true
        }
    }
}

fn actor_loop(
    rx: std::sync::mpsc::Receiver<Job>,
    wake: std::sync::Arc<WakeEvent>,
    mut conn: Option<Conn>,
) {
    loop {
        if conn.is_none() {
            // No connection, nothing to serve: block on the queue. A tick
            // here would be a pure wakeup tax - with Discord closed the
            // old loop still woke ~50x/s for literally nothing.
            match rx.recv() {
                Ok(job) => {
                    if !handle_job(&mut conn, job) {
                        break;
                    }
                }
                Err(_) => break,
            }
            continue;
        }
        // Connected: keep the session honest (answer pings, absorb
        // voice-settings pushes). `wake` ends the serve the moment
        // execute() queues a job, so the long tick costs no action
        // latency. A transport failure drops the session so the next
        // action reconnects instead of failing.
        if conn
            .as_mut()
            .unwrap()
            .serve(IDLE_TICK, wake.handle())
            .is_err()
        {
            conn = None;
            continue;
        }
        // Serve returned (tick elapsed or wake fired): drain the queue
        // without blocking again.
        loop {
            match rx.try_recv() {
                Ok(job) => {
                    if !handle_job(&mut conn, job) {
                        return;
                    }
                    if conn.is_none() {
                        // the job killed the connection; go back to
                        // blocking on the queue
                        break;
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
            }
        }
    }
}

/// Connect on demand, run the plan, and drop the session on any failure so
/// the next action starts fresh (an expired token surfaces as AuthRejected
/// and goes through the caller's re-authorization path).
fn run_exec(
    conn: &mut Option<Conn>,
    config: &DiscordConfig,
    action: &str,
    args: &Value,
    deadline: Instant,
) -> Result<Option<ExecOutcome>> {
    let what = plan(action, args)?;
    if conn.is_none() {
        *conn = Some(Conn::connect(config, deadline)?);
    }
    let outcome = run_connected(conn.as_mut().unwrap(), &what, deadline);
    if outcome.is_err() {
        *conn = None;
    }
    outcome
}

fn run_connected(conn: &mut Conn, what: &Plan, deadline: Instant) -> Result<Option<ExecOutcome>> {
    match what {
        Plan::ConnectChannel(id) => {
            let reply = conn.request_until(
                deadline,
                "SELECT_VOICE_CHANNEL",
                json!({ "channel_id": id, "timeout": 30i32 }),
            )?;
            require_ok(&reply)?;
            Ok(None)
        }
        Plan::Disconnect => {
            let reply = conn.request_until(
                deadline,
                "SELECT_VOICE_CHANNEL",
                json!({ "channel_id": Value::Null, "timeout": 30i32 }),
            )?;
            require_ok(&reply)?;
            Ok(None)
        }
        _ => {
            // Flip-style plans need the current state. The cache kept fresh
            // by VOICE_SETTINGS_UPDATE pushes avoids a GET round trip; only
            // a cold cache (fresh connection) pays for one.
            if conn.cache.is_none() {
                let reply = conn.request_until(deadline, "GET_VOICE_SETTINGS", json!({}))?;
                require_ok(&reply)?;
                if let Some(data) = reply.get("data").filter(|v| v.is_object()) {
                    conn.cache = Some(data.clone());
                }
            }
            let Some(current) = conn.cache.as_ref() else {
                return Err(DiscordError::Call("GET_VOICE_SETTINGS"));
            };
            let Some(outcome) = apply_flip(current, what)? else {
                return Ok(None);
            };
            let reply = conn.request_until(deadline, "SET_VOICE_SETTINGS", outcome.patch)?;
            require_ok(&reply)?;
            // the reply carries the authoritative post-set state
            if let Some(data) = reply.get("data").filter(|v| v.is_object()) {
                conn.cache = Some(data.clone());
            }
            if outcome.label.is_empty() {
                return Ok(None);
            }
            let key = if matches!(what, Plan::FlipMic | Plan::SetMic(_)) {
                "toggle-microphone"
            } else {
                "toggle-headphone"
            };
            Ok(Some(ExecOutcome {
                key: key.to_string(),
                label: outcome.label.to_string(),
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_status_without_config_or_token_needs_no_network() {
        // both branches return before any pipe/HTTP dial: safe to assert
        // on a machine without Discord
        assert_eq!(
            probe_status(&DiscordConfig::default()),
            DiscordStatus::NotConfigured
        );
        assert_eq!(
            probe_status(&DiscordConfig {
                client_id: "123".into(),
                ..Default::default()
            }),
            DiscordStatus::NeedsAuth
        );
    }

    #[test]
    fn save_config_and_clear_tokens_preserve_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(
            &path,
            r#"{
                "foreign-ext": { "custom": { "value": "keep-me" } },
                "discord-deckboard": {
                    "discordAccessToken": { "value": "old-token" },
                    "discordRefreshToken": { "value": "old-refresh" }
                }
            }"#,
        )
        .unwrap();

        save_config(&path, "my-client-id", Some("my-client-secret")).unwrap();
        let val: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(val["foreign-ext"]["custom"]["value"], "keep-me");
        assert_eq!(
            val["discord-deckboard"]["discordClientId"]["value"],
            "my-client-id"
        );
        assert_eq!(
            val["discord-deckboard"]["discordClientSecret"]["value"],
            "my-client-secret"
        );
        assert_eq!(
            val["discord-deckboard"]["discordAccessToken"]["value"],
            "old-token"
        );

        // None keeps the stored secret (the UI never echoes it, so an
        // empty secret input must not erase the saved one)
        save_config(&path, "changed-id", None).unwrap();
        let val_kept: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            val_kept["discord-deckboard"]["discordClientId"]["value"],
            "changed-id"
        );
        assert_eq!(
            val_kept["discord-deckboard"]["discordClientSecret"]["value"], "my-client-secret",
            "None must keep the stored secret"
        );

        clear_tokens(&path).unwrap();
        let val2: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(val2["foreign-ext"]["custom"]["value"], "keep-me");
        assert_eq!(
            val2["discord-deckboard"]["discordClientId"]["value"], "changed-id",
            "clearing tokens must keep the credentials"
        );
        assert_eq!(
            val2["discord-deckboard"]["discordClientSecret"]["value"],
            "my-client-secret"
        );
        assert_eq!(val2["discord-deckboard"]["discordAccessToken"]["value"], "");
        assert_eq!(
            val2["discord-deckboard"]["discordRefreshToken"]["value"],
            ""
        );
    }

    #[test]
    fn transport_failures_are_network_not_rejections() {
        // port 9 (discard) on loopback refuses instantly: the request
        // never reaches a Discord endpoint, so the error must be Network -
        // callers must be able to tell "offline" from "Discord said no"
        // and skip the consent popup while offline.
        let err = post_form("http://127.0.0.1:9/oauth2/token", "a=b").unwrap_err();
        assert!(matches!(err, DiscordError::Network(_)), "got {err:?}");
        // without a refresh token there is nothing to try silently: that
        // is a real rejection, the interactive popup is the correct answer
        let cfg = DiscordConfig {
            client_id: "id".into(),
            client_secret: "sec".into(),
            access_token: "t".into(),
            refresh_token: None,
        };
        assert!(matches!(refresh(&cfg), Err(DiscordError::AuthRejected)));
    }

    #[test]
    fn frame_roundtrip() {
        let enc = encode_frame(OP_FRAME, r#"{"a":1}"#);
        assert_eq!(enc.len(), 8 + 7);
        assert_eq!(&enc[0..4], &1i32.to_le_bytes());
        assert_eq!(&enc[4..8], &7i32.to_le_bytes());
        let (op, payload) = decode_frame(&enc).unwrap();
        assert_eq!(op, OP_FRAME);
        assert_eq!(payload, br#"{"a":1}"#);
        assert!(decode_frame(&enc[..10]).is_none());
    }

    #[test]
    fn extract_frames_splits_concatenated_and_keeps_partials() {
        let a = encode_frame(OP_FRAME, r#"{"a":1}"#);
        let b = encode_frame(OP_PING, r#"{"b":"x"}"#);
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(&a);
        buf.extend_from_slice(&b);
        // a partial frame behind two complete ones must survive the pass
        buf.extend_from_slice(&a[..5]);
        let frames = extract_frames(&mut buf);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].0, OP_FRAME);
        assert_eq!(frames[1].0, OP_PING);
        assert_eq!(buf.len(), 5);
        buf.extend_from_slice(&a[5..]);
        let frames = extract_frames(&mut buf);
        assert_eq!(frames.len(), 1);
        assert!(buf.is_empty());
    }

    #[test]
    fn extract_frames_drops_non_json_payloads() {
        let mut buf = encode_frame(OP_PING, "not json");
        buf.extend_from_slice(&encode_frame(OP_FRAME, r#"{"a":2}"#));
        let frames = extract_frames(&mut buf);
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].1["a"], 2);
        assert!(buf.is_empty());
    }

    #[test]
    fn classify_routes_replies_events_pings_and_noise() {
        let reply = json!({"cmd":"SET_VOICE_SETTINGS","nonce":"dk-7","data":{"mute":false}});
        assert_eq!(classify(OP_FRAME, &reply, Some("dk-7")), Incoming::Reply);
        // error frames carry our nonce too: the caller inspects evt
        let error =
            json!({"cmd":"AUTHENTICATE","evt":"ERROR","nonce":"dk-2","data":{"message":"bad"}});
        assert_eq!(classify(OP_FRAME, &error, Some("dk-2")), Incoming::Reply);
        let push = json!({"cmd":"DISPATCH","evt":"VOICE_SETTINGS_UPDATE","data":{"mute":true}});
        match classify(OP_FRAME, &push, None) {
            Incoming::VoiceSettings(v) => assert_eq!(v["mute"], true),
            other => panic!("expected voice settings push, got {other:?}"),
        }
        match classify(OP_PING, &json!({ "z": 1 }), None) {
            Incoming::Ping(_) => {}
            other => panic!("expected ping, got {other:?}"),
        }
        let ready = json!({"cmd":"DISPATCH","evt":"READY"});
        assert_eq!(classify(OP_FRAME, &ready, None), Incoming::Other);
        // someone else's reply is not ours
        let other_nonce = json!({"cmd":"X","nonce":"dk-1"});
        assert_eq!(
            classify(OP_FRAME, &other_nonce, Some("dk-9")),
            Incoming::Other
        );
        // idle serving never claims a reply, whatever the frame carries
        assert_eq!(classify(OP_FRAME, &reply, None), Incoming::Other);
    }

    #[test]
    fn error_replies_are_rejected_not_treated_as_data() {
        let ok = json!({"cmd":"SET_VOICE_SETTINGS","nonce":"dk-1","data":{"mute":false}});
        assert!(require_ok(&ok).is_ok());
        // error frames carry their payload in `data` too; require_ok must
        // send them down the error path so they never reach the cache
        let err = json!({"cmd":"SET_VOICE_SETTINGS","evt":"ERROR","nonce":"dk-1","data":{"code":4000,"message":"bad"}});
        assert!(require_ok(&err).is_err());
    }

    #[test]
    fn config_reads_original_settings_shape() {
        let settings = json!({
            "discord-deckboard": {
                "discordClientId": { "value": "123" },
                "discordAccessToken": { "value": "tok" },
            }
        });
        let cfg = DiscordConfig::from_settings(&settings).unwrap();
        assert_eq!(cfg.client_id, "123");
        assert_eq!(cfg.access_token, "tok");
        assert!(DiscordConfig::from_settings(&json!({})).is_none());
        // empty token still yields a config: the app is set up but the
        // popup authorization has not happened yet
        let empty = json!({
            "discord-deckboard": {
                "discordClientId": { "value": "123" },
                "discordAccessToken": { "value": "" },
            }
        });
        let cfg = DiscordConfig::from_settings(&empty).unwrap();
        assert!(cfg.access_token.is_empty());
    }

    #[test]
    fn save_tokens_errors_on_unmergeable_settings() {
        // save_tokens runs inside a background re-auth task: valid JSON
        // that is not an object (or a non-object `discord-deckboard`
        // field) must surface as an error, not as a panic swallowed into
        // a JoinError.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let tokens = AuthTokens {
            access_token: "a".into(),
            refresh_token: None,
        };

        std::fs::write(&path, "[1,2,3]").unwrap();
        let err = save_tokens(&path, &tokens).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);

        std::fs::write(&path, r#"{"discord-deckboard": 5}"#).unwrap();
        assert!(save_tokens(&path, &tokens).is_err());

        // happy shape still persists
        std::fs::write(&path, "{}").unwrap();
        save_tokens(&path, &tokens).unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved["discord-deckboard"]["discordAccessToken"]["value"],
            "a"
        );
    }

    #[test]
    fn save_tokens_refuses_to_touch_a_corrupt_settings_file() {
        // a settings.json that exists but does not parse must not be the
        // start of a fresh `{}`: silently merging over it would wipe every
        // other extension's config. Error and leave the bytes untouched.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let tokens = AuthTokens {
            access_token: "fake-access-token".into(),
            refresh_token: None,
        };

        std::fs::write(&path, b"{\"discord-deckboard\": {truncated").unwrap();
        let err = save_tokens(&path, &tokens).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"{\"discord-deckboard\": {truncated",
            "the corrupt file must be left exactly as it was"
        );
        assert!(
            !dir.path().join("settings.json.tmp").exists(),
            "no temp file may be left behind by the refused write"
        );
    }

    #[test]
    fn save_tokens_preserves_unrelated_settings_and_creates_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let tokens = AuthTokens {
            access_token: "fake-access-token".into(),
            refresh_token: Some("fake-refresh-token".into()),
        };

        // missing file: the only case that starts from `{}`
        save_tokens(&path, &tokens).unwrap();
        let created: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            created["discord-deckboard"]["discordRefreshToken"]["value"],
            "fake-refresh-token"
        );

        // an existing file keeps every unrelated field
        std::fs::write(
            &path,
            r#"{"other-package":{"someSetting":{"value":"keep-me"}}}"#,
        )
        .unwrap();
        save_tokens(&path, &tokens).unwrap();
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            saved["other-package"]["someSetting"]["value"], "keep-me",
            "merging must not wipe unrelated extensions' settings"
        );
        assert_eq!(
            saved["discord-deckboard"]["discordAccessToken"]["value"],
            "fake-access-token"
        );
    }

    #[test]
    fn plans_cover_tile_commands() {
        // shapes exactly as stored in the user's database
        assert_eq!(
            plan("microphone", &json!({ "action": "toggle_microphone" })).unwrap(),
            Plan::FlipMic
        );
        assert_eq!(
            plan("headphone", &json!({ "action": "toggle_headphone" })).unwrap(),
            Plan::FlipDeaf
        );
        assert_eq!(
            plan("disconnect-voice", &json!({})).unwrap(),
            Plan::Disconnect
        );
        assert_eq!(
            plan(
                "connect-voice",
                &json!({ "channel_id": "1348374896685875295" })
            )
            .unwrap(),
            Plan::ConnectChannel("1348374896685875295".into())
        );
        assert!(plan("microphone", &json!({ "action": "nonsense" })).is_err());
    }

    #[test]
    fn flips_match_original_labels() {
        let muted = json!({ "mute": true, "deaf": false });
        // muted mic flipped -> unmuted, and the original labels the NEW
        // state: unmuted shows "ON"
        let out = apply_flip(&muted, &Plan::FlipMic).unwrap().unwrap();
        assert_eq!(out.label, "ON");
        assert_eq!(out.patch, json!({ "mute": false }));
        // deaf currently false -> flip turns it on, label OFF
        let out = apply_flip(&muted, &Plan::FlipDeaf).unwrap().unwrap();
        assert_eq!(out.label, "OFF");
        assert_eq!(out.patch, json!({ "deaf": true }));
        // explicit set keeps the label semantics
        let out = apply_flip(&muted, &Plan::SetMic(false)).unwrap().unwrap();
        assert_eq!(out.label, "ON");
        // channel plans never produce a patch
        assert!(apply_flip(&muted, &Plan::Disconnect).unwrap().is_none());
    }

    #[test]
    fn action_routing() {
        for kind in ["toggle-microphone", "connect-voice", "change-input"] {
            assert!(is_discord_action(kind));
        }
        assert!(!is_discord_action("vm-restart"));
        assert!(!is_discord_action("key"));
    }

    #[test]
    fn pipe_probe_is_harmless_when_discord_absent() {
        // if Discord runs this opens a real pipe and drops it again; either
        // way it must not panic
        let _ = Pipe::open();
    }

    #[test]
    fn config_debug_redacts_credentials() {
        // logs are kept for 14 days; the derived Debug would have printed
        // every credential verbatim
        let config = DiscordConfig {
            client_id: "123456789012345678".into(),
            client_secret: "fake-client-secret".into(),
            access_token: "fake-access-token".into(),
            refresh_token: Some("fake-refresh-token".into()),
        };
        let printed = format!("{config:?}");
        assert!(!printed.contains("fake-client-secret"));
        assert!(!printed.contains("fake-access-token"));
        assert!(!printed.contains("fake-refresh-token"));
        assert!(
            printed.contains("123456789012345678"),
            "client id is public"
        );
    }

    #[test]
    fn auth_tokens_debug_redacts_credentials() {
        let tokens = AuthTokens {
            access_token: "fake-access-token".into(),
            refresh_token: Some("fake-refresh-token".into()),
        };
        let printed = format!("{tokens:?}");
        assert!(!printed.contains("fake-access-token"));
        assert!(!printed.contains("fake-refresh-token"));
    }

    #[test]
    fn extract_frames_drops_a_buffer_with_an_absurd_frame_length() {
        // a corrupted or hostile length header must not let the receive
        // buffer grow toward the claimed size (up to 2 GiB)
        let mut buf = encode_frame(OP_FRAME, "{}");
        buf[4..8].copy_from_slice(&i32::MAX.to_le_bytes());
        buf.extend_from_slice(b"junk-after-the-lie");
        let frames = extract_frames(&mut buf);
        assert!(frames.is_empty());
        assert!(
            buf.is_empty(),
            "the poisoned buffer is dropped, not retained"
        );
    }

    #[test]
    fn http_agent_times_out_against_a_silent_server() {
        // accept the connection, then never answer: an agent without a
        // timeout blocks the calling action thread until OS defaults
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            let _ = listener.accept();
        });
        let agent = http_agent(Duration::from_millis(300));
        let started = Instant::now();
        let result = agent.get(format!("http://{addr}/")).call();
        assert!(result.is_err());
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the agent must carry a real timeout, got {:?}",
            started.elapsed()
        );
    }

    /// Stand-in Discord: one named-pipe server instance that accepts a
    /// client and then never speaks, so the actor's connection sits in an
    /// idle overlapped read for the whole test.
    fn spawn_silent_pipe_server(name: &str) {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(Some(0))
            .collect();
        let pipe_access_duplex: u32 = 0x3;
        // SAFETY: name is NUL-terminated and lives across the call; the
        // server handle is deliberately leaked - the test process exits
        // right after
        let server = unsafe {
            pipe::CreateNamedPipeW(
                wide.as_ptr(),
                pipe_access_duplex,
                0, // byte mode, wait
                1,
                4096,
                4096,
                0,
                std::ptr::null_mut(),
            )
        };
        assert!(
            !server.is_null() && server != pipe::INVALID_HANDLE_VALUE,
            "CreateNamedPipeW failed"
        );
        // raw handles are not Send; this server handle is only ever passed
        // to blocking kernel waits on this one thread
        struct ServerHandle(*mut c_void);
        unsafe impl Send for ServerHandle {}
        impl ServerHandle {
            fn raw(self) -> *mut c_void {
                self.0
            }
        }
        let server = ServerHandle(server);
        std::thread::spawn(move || {
            // a sync server handle: ConnectNamedPipe blocks until the
            // client shows up
            unsafe { pipe::ConnectNamedPipe(server.raw(), std::ptr::null_mut()) };
            std::thread::sleep(Duration::from_secs(60));
        });
    }

    #[test]
    fn execute_completes_while_the_actor_serves_a_silent_connection() {
        // The old loop only noticed a queued job after its 20 ms tick; a
        // connected session must instead be interrupted immediately, no
        // matter how long the idle tick is (now 10 s).
        let name = format!(
            "\\\\.\\pipe\\pulpit-discord-wake-test-{}-{}",
            std::process::id(),
            line!()
        );
        spawn_silent_pipe_server(&name);
        let conn = Conn {
            pipe: open_test_pipe(&name),
            nonce: 0,
            buf: Vec::new(),
            inbox: VecDeque::new(),
            cache: None,
            username: None,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let wake = std::sync::Arc::new(WakeEvent::new());
        let thread_wake = wake.clone();
        std::thread::spawn(move || actor_loop(rx, thread_wake, Some(conn)));
        let client = DiscordClient {
            jobs: std::sync::Mutex::new(tx),
            wake,
        };
        let cfg = DiscordConfig {
            client_id: "id".into(),
            client_secret: "sec".into(),
            access_token: "t".into(),
            refresh_token: None,
        };
        let started = Instant::now();
        // an unknown action fails at plan() - before any pipe I/O - so the
        // test isolates exactly one property: the queued job interrupted
        // the idle serve and was answered, instead of waiting out the tick
        let result = client.execute(
            &cfg,
            "not-an-action",
            &json!({}),
            Instant::now() + Duration::from_secs(5),
        );
        assert!(
            matches!(result, Err(DiscordError::BadPayload(_, _))),
            "got {result:?}"
        );
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the wake must interrupt the idle serve immediately, took {:?}",
            started.elapsed()
        );
        // a second action still flows after the first interrupted serve
        let started = Instant::now();
        let result = client.execute(
            &cfg,
            "not-an-action",
            &json!({}),
            Instant::now() + Duration::from_secs(5),
        );
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(3));
    }

    /// Client end of a test pipe, opened like the real Discord pipe.
    fn open_test_pipe(name: &str) -> Pipe {
        use std::os::windows::ffi::OsStrExt;
        let path: Vec<u16> = std::ffi::OsStr::new(name)
            .encode_wide()
            .chain(Some(0))
            .collect();
        // SAFETY: path is NUL-terminated and lives across the call
        let handle = unsafe {
            pipe::CreateFileW(
                path.as_ptr(),
                pipe::GENERIC_READ_WRITE,
                0,
                std::ptr::null_mut(),
                pipe::OPEN_EXISTING,
                pipe::FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        assert!(
            handle != pipe::INVALID_HANDLE_VALUE && !handle.is_null(),
            "client connect to the test pipe failed"
        );
        Pipe { handle }
    }

    /// Live probe: reads the saved token from settings.json, connects and
    /// authenticates only (no state changes). Run explicitly:
    /// `cargo test -p pulpit-discord -- --ignored --nocapture live_auth`
    #[test]
    #[ignore = "talks to the real Discord pipe"]
    fn live_authenticate() {
        let config = saved_config();
        let conn =
            Conn::connect(&config, Instant::now() + Duration::from_secs(8)).expect("auth failed");
        drop(conn);
        println!("authenticated ok");
    }

    /// Live probe of click latency through the kept-alive client. Runs two
    /// mic flips and two deaf flips, so Discord ends in the state it
    /// started in. Run explicitly:
    /// `cargo test -p pulpit-discord -- --ignored --nocapture live_latency`
    #[test]
    #[ignore = "talks to the real Discord pipe"]
    fn live_latency_probe() {
        let config = saved_config();
        let client = DiscordClient::spawn();
        let toggles = [
            ("microphone", json!({ "action": "toggle_microphone" })),
            ("microphone", json!({ "action": "toggle_microphone" })),
            ("headphone", json!({ "action": "toggle_headphone" })),
            ("headphone", json!({ "action": "toggle_headphone" })),
        ];
        for (round, (kind, args)) in toggles.iter().enumerate() {
            let t = Instant::now();
            client
                .execute(&config, kind, args, Instant::now() + Duration::from_secs(8))
                .expect("execute failed");
            println!("click {}: {:?}", round + 1, t.elapsed());
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    fn saved_config() -> DiscordConfig {
        let home = std::env::var("USERPROFILE").unwrap();
        let raw = std::fs::read_to_string(format!("{home}\\deckboard\\settings.json"))
            .or_else(|_| std::fs::read_to_string(format!("{home}\\pulpitApp\\settings.json")))
            .expect("settings.json not found");
        DiscordConfig::from_settings(&serde_json::from_str(&raw).unwrap())
            .expect("no discord credentials in settings.json")
    }
}
