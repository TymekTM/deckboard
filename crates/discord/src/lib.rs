//! Native Discord local-RPC integration.
//!
//! Replaces the `discord-deckboard` extension, which needs node sockets to
//! reach Discord's named pipe. The protocol (from discord-rpc, mirrored
//! here): connect to `\\?\pipe\discord-ipc-N`, send a handshake frame
//! (opcode 0) with the OAuth2 client id, authenticate with the access
//! token the user saved in the original app, then talk commands (opcode 1)
//! - GET/SET_VOICE_SETTINGS and SELECT_VOICE_CHANNEL. Frames are
//! `[i32 LE opcode][i32 LE length][json]`.
//!
//! A connection is opened per action and closed after it: cheap, and it
//! keeps the backend stateless.

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
    #[error("Discord RPC call failed: {0}")]
    Call(&'static str),
    #[error("bad payload for {0}: {1}")]
    BadPayload(&'static str, String),
}

pub type Result<T> = std::result::Result<T, DiscordError>;

/// Credentials read from `~/deckboard/settings.json` (the original app's
/// config fields for the discord-deckboard package).
#[derive(Debug, Clone, Default)]
pub struct DiscordConfig {
    pub client_id: String,
    pub client_secret: String,
    pub access_token: String,
    pub refresh_token: Option<String>,
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

/// Discord's local HTTP endpoint (the port that answers 404 on /).
fn find_endpoint() -> Result<String> {
    let agent = http_agent();
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
/// API answers 404/400 on purpose).
fn http_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .build()
        .new_agent()
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
    let resp = http_agent()
        .post(url)
        .content_type("application/x-www-form-urlencoded")
        .header("Authorization", "Bearer null") // discord-rpc sends this too
        .send(body.as_bytes())
        .map_err(|_| DiscordError::Call("oauth request"))?;
    let text = resp
        .into_body()
        .read_to_string()
        .map_err(|_| DiscordError::Call("oauth body"))?;
    let parsed: Value = serde_json::from_str(&text).map_err(|_| DiscordError::Call("oauth json"))?;
    if parsed.get("access_token").is_none() {
        // {"error": "invalid_client", ...}
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
    let mut pipe = Pipe::open()?;
    pipe.write_all(
        &encode_frame(
            OP_HANDSHAKE,
            &json!({ "v": 1, "client_id": config.client_id }).to_string(),
        ),
    )?;
    let mut session = Session { pipe, nonce: 0 };
    session.wait_for_ready(deadline)?;
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
    let raw = std::fs::read_to_string(path).unwrap_or_else(|_| "{}".into());
    let mut settings: Value =
        serde_json::from_str(&raw).unwrap_or_else(|_| Value::Object(Default::default()));
    let obj = settings.as_object_mut().expect("settings object");
    let package = obj
        .entry("discord-deckboard")
        .or_insert_with(|| Value::Object(Default::default()));
    let package = package.as_object_mut().expect("package object");
    let mut field = |name: &str, value: &str| {
        let entry = package
            .entry(name.to_string())
            .or_insert_with(|| {
                json!({
                    "descriptions": "Discord OAuth token (managed by deckboard-server)",
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
    std::fs::write(path, serde_json::to_string_pretty(&settings).unwrap_or_default())
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
pub fn input_declarations() -> Vec<(&'static str, &'static str, &'static str, Option<&'static str>)> {
    vec![
        ("toggle-microphone", "microphone", "#5865F2", Some("custom-value")),
        ("toggle-headphone", "headphones", "#5865F2", Some("custom-value")),
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

pub fn encode_frame(op: u32, payload: &str) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 + payload.len());
    buf.extend_from_slice(&(op as i32).to_le_bytes());
    buf.extend_from_slice(&(payload.len() as i32).to_le_bytes());
    buf.extend_from_slice(payload.as_bytes());
    buf
}

/// Parse `[op][len][payload]` from a buffer; None while incomplete.
pub fn decode_frame(buf: &[u8]) -> Option<(u32, &[u8])> {
    if buf.len() < 8 {
        return None;
    }
    let op = u32::from_le_bytes(buf[0..4].try_into().ok()?) as i32;
    let len = i32::from_le_bytes(buf[4..8].try_into().ok()?).max(0) as usize;
    if buf.len() < 8 + len {
        return None;
    }
    Some((op as u32, &buf[8..8 + len]))
}

// ------------------------------------------------------------- pipe client

#[repr(C)]
struct OverlappedPlaceholder;

#[cfg(windows)]
mod pipe {
    use std::ffi::c_void;

    pub const INVALID_HANDLE_VALUE: *mut c_void = -1isize as *mut c_void;
    pub const GENERIC_READ_WRITE: u32 = 0x8000_0000 | 0x4000_0000;
    pub const OPEN_EXISTING: u32 = 3;

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
            overlapped: *mut c_void,
        ) -> i32;
        pub fn WriteFile(
            handle: *mut c_void,
            buffer: *const u8,
            to_write: u32,
            written: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        pub fn PeekNamedPipe(
            handle: *mut c_void,
            buffer: *mut u8,
            buffer_size: u32,
            read: *mut u32,
            available: *mut u32,
            left: *mut u32,
        ) -> i32;
        pub fn CloseHandle(handle: *mut c_void) -> i32;
    }
}

struct Pipe {
    handle: *mut c_void,
}

// SAFETY: the raw handle is owned exclusively by this struct.
unsafe impl Send for Pipe {}

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
                    0,
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
            let mut written = 0u32;
            // SAFETY: buffer outlives the call, written is a valid out-param
            let ok = unsafe {
                pipe::WriteFile(
                    self.handle,
                    data.as_ptr(),
                    data.len().min(u32::MAX as usize) as u32,
                    &mut written,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || written == 0 {
                return Err(DiscordError::Call("WriteFile"));
            }
            data = &data[written as usize..];
        }
        Ok(())
    }

    fn read_exact(&mut self, out: &mut [u8], deadline: Instant) -> Result<()> {
        let mut done = 0;
        while done < out.len() {
            if Instant::now() > deadline {
                return Err(DiscordError::Call("timeout"));
            }
            let mut available = 0u32;
            // SAFETY: only the available count is consumed
            let peek = unsafe {
                pipe::PeekNamedPipe(
                    self.handle,
                    std::ptr::null_mut(),
                    0,
                    std::ptr::null_mut(),
                    &mut available,
                    std::ptr::null_mut(),
                )
            };
            if peek == 0 {
                return Err(DiscordError::Call("PeekNamedPipe"));
            }
            if available == 0 {
                std::thread::sleep(Duration::from_millis(20));
                continue;
            }
            let mut chunk = [0u8; 4096];
            let want = out.len() - done;
            let mut read = 0u32;
            // SAFETY: chunk is a valid receive buffer, read the out-param
            let ok = unsafe {
                pipe::ReadFile(
                    self.handle,
                    chunk.as_mut_ptr(),
                    chunk.len().min(want) as u32,
                    &mut read,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 || read == 0 {
                return Err(DiscordError::Call("ReadFile"));
            }
            out[done..done + read as usize].copy_from_slice(&chunk[..read as usize]);
            done += read as usize;
        }
        Ok(())
    }
}

impl Drop for Pipe {
    fn drop(&mut self) {
        // SAFETY: handle is a live handle we own
        unsafe { pipe::CloseHandle(self.handle) };
    }
}

// ------------------------------------------------------------------- calls

/// One authenticated Discord RPC session.
pub struct Session {
    pipe: Pipe,
    nonce: u64,
}

impl Session {
    pub fn connect(config: &DiscordConfig) -> Result<Session> {
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
        let mut pipe = Pipe::open()?;
        let deadline = Instant::now() + Duration::from_secs(8);
        pipe.write_all(
            &encode_frame(
                OP_HANDSHAKE,
                &json!({ "v": 1, "client_id": config.client_id }).to_string(),
            ),
        )?;
        let mut session = Session { pipe, nonce: 0 };
        // Discord ignores commands sent before READY was consumed, so read
        // the dispatch first
        session.wait_for_ready(deadline)?;
        let reply = session.request_until(
            deadline,
            "AUTHENTICATE",
            json!({ "access_token": config.access_token }),
        )?;
        if reply.get("evt").and_then(Value::as_str) == Some("ERROR") {
            return Err(DiscordError::AuthRejected);
        }
        Ok(session)
    }

    fn next_nonce(&mut self) -> String {
        self.nonce += 1;
        format!("dk-{}", self.nonce)
    }

    /// Read one complete frame, answering pings on the way.
    fn read_frame(&mut self, deadline: Instant) -> Result<(u32, Value)> {
        let mut buf: Vec<u8> = Vec::new();
        loop {
            if let Some((op, payload)) = decode_frame(&buf) {
                let payload = payload.to_vec();
                let len = payload.len();
                let parsed: Value = serde_json::from_slice(&payload)
                    .map_err(|_| DiscordError::Call("bad json frame"))?;
                buf.drain(..8 + len);
                match op {
                    OP_PING => {
                        let text = String::from_utf8_lossy(&payload).into_owned();
                        self.pipe.write_all(&encode_frame(OP_PONG, &text))?;
                        continue;
                    }
                    OP_FRAME => return Ok((op, parsed)),
                    _ => continue,
                }
            }
            // incomplete frame: read the header first, then its payload
            let need = if buf.len() < 8 {
                8 - buf.len()
            } else {
                let len = i32::from_le_bytes(buf[4..8].try_into().unwrap()).max(0) as usize;
                8 + len - buf.len()
            };
            let mut tmp = vec![0u8; need];
            self.pipe.read_exact(&mut tmp, deadline)?;
            buf.extend_from_slice(&tmp);
        }
    }

    /// The first dispatch after the handshake (cmd DISPATCH, evt READY).
    fn wait_for_ready(&mut self, deadline: Instant) -> Result<()> {
        let (op, frame) = self.read_frame(deadline)?;
        let is_ready = op == OP_FRAME
            && frame["cmd"].as_str() == Some("DISPATCH")
            && frame["evt"].as_str() == Some("READY");
        if is_ready {
            Ok(())
        } else {
            Err(DiscordError::Call("expected READY dispatch"))
        }
    }

    fn request_until(
        &mut self,
        deadline: Instant,
        cmd: &str,
        args: Value,
    ) -> Result<Value> {
        let nonce = self.next_nonce();
        let frame = json!({ "cmd": cmd, "args": args, "nonce": nonce });
        self.pipe.write_all(&encode_frame(OP_FRAME, &frame.to_string()))?;
        loop {
            let (op, v) = self.read_frame(deadline)?;
            if op == OP_FRAME && v.get("nonce").and_then(Value::as_str) == Some(nonce.as_str()) {
                return Ok(v);
            }
            // unrelated dispatch/event - keep reading
        }
    }

    pub fn get_voice_settings(&mut self, deadline: Instant) -> Result<Value> {
        let reply = self.request_until(deadline, "GET_VOICE_SETTINGS", json!({}))?;
        reply
            .get("data")
            .cloned()
            .ok_or(DiscordError::Call("GET_VOICE_SETTINGS"))
    }

    pub fn set_voice_settings(&mut self, patch: Value, deadline: Instant) -> Result<()> {
        self.request_until(deadline, "SET_VOICE_SETTINGS", patch)?;
        Ok(())
    }

    pub fn select_voice_channel(
        &mut self,
        channel_id: Option<&str>,
        deadline: Instant,
    ) -> Result<()> {
        self.request_until(
            deadline,
            "SELECT_VOICE_CHANNEL",
            json!({ "channel_id": channel_id, "timeout": 30i32 }),
        )?;
        Ok(())
    }
}

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
        Plan::SetMic(mute) => Some(FlipOutcome { label: label(*mute), patch: json!({ "mute": mute }) }),
        Plan::SetDeaf(deaf) => Some(FlipOutcome { label: label(*deaf), patch: json!({ "deaf": deaf }) }),
        Plan::FlipMic => {
            let now = get("mute")?;
            Some(FlipOutcome { label: label(!now), patch: json!({ "mute": !now }) })
        }
        Plan::FlipDeaf => {
            let now = get("deaf")?;
            Some(FlipOutcome { label: label(!now), patch: json!({ "deaf": !now }) })
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
            Some(FlipOutcome { label: "", patch: json!({ "mode": { "type": next } }) })
        }
        Plan::ConnectChannel(_) | Plan::Disconnect => None,
    })
}

/// Execute one action end to end; pushed labels go through `push`.
pub fn execute(
    config: &DiscordConfig,
    action: &str,
    args: &Value,
    mut push: impl FnMut(String, String),
) -> Result<()> {
    let what = plan(action, args)?;
    if let Plan::ConnectChannel(id) = &what {
        let mut s = Session::connect(config)?;
        return s.select_voice_channel(Some(id), Instant::now() + Duration::from_secs(5));
    }
    if let Plan::Disconnect = &what {
        let mut s = Session::connect(config)?;
        return s.select_voice_channel(None, Instant::now() + Duration::from_secs(5));
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut s = Session::connect(config)?;
    let current = s.get_voice_settings(deadline)?;
    if let Some(outcome) = apply_flip(&current, &what)? {
        s.set_voice_settings(outcome.patch, deadline)?;
        if !outcome.label.is_empty() {
            let key = if matches!(what, Plan::FlipMic | Plan::SetMic(_)) {
                "toggle-microphone"
            } else {
                "toggle-headphone"
            };
            push(key.to_string(), outcome.label.to_string());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert_eq!(plan("disconnect-voice", &json!({})).unwrap(), Plan::Disconnect);
        assert_eq!(
            plan("connect-voice", &json!({ "channel_id": "1348374896685875295" })).unwrap(),
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

    /// Live probe: reads the saved token from settings.json and runs
    /// handshake + AUTHENTICATE only (no state changes). Run explicitly:
    /// `cargo test -p deckboard-discord -- --ignored`
    #[test]
    #[ignore = "talks to the real Discord pipe"]
    fn live_authenticate() {
        // first: bogus id must produce an immediate error frame - proves
        // the frame codec reads real replies
        let mut pipe = Pipe::open().expect("no discord pipe");
        let deadline = Instant::now() + Duration::from_secs(3);
        pipe.write_all(&encode_frame(
            OP_HANDSHAKE,
            &json!({ "v": 1, "client_id": "0" }).to_string(),
        ))
        .unwrap();
        let mut tmp = vec![0u8; 8];
        pipe.read_exact(&mut tmp, deadline).unwrap();
        let len = i32::from_le_bytes(tmp[4..8].try_into().unwrap()) as usize;
        let mut rest = vec![0u8; len];
        pipe.read_exact(&mut rest, deadline).unwrap();
        println!("bogus handshake reply: {}", String::from_utf8_lossy(&rest));
        drop(pipe);

        // now the real credentials
        let home = std::env::var("USERPROFILE").unwrap();
        let raw = std::fs::read_to_string(format!("{home}\\deckboard\\settings.json")).unwrap();
        let settings: Value = serde_json::from_str(&raw).unwrap();
        let config = DiscordConfig::from_settings(&settings)
            .expect("no discord credentials in settings.json");
        let mut session = Session::connect(&config).expect("authenticate failed");
        let settings = session
            .get_voice_settings(Instant::now() + Duration::from_secs(5))
            .unwrap();
        println!(
            "authenticated; voice settings keys: {:?}",
            settings.as_object().map(|o| o.keys().collect::<Vec<_>>())
        );
    }

    /// Manual probe of the real handshake path; prints diagnostics only.
    #[test]
    #[ignore = "manual probe, prints settings fingerprints"]
    fn live_raw_real_id() {
        let home = std::env::var("USERPROFILE").unwrap();
        let path = format!("{home}\\deckboard\\settings.json");
        let raw = std::fs::read_to_string(&path).unwrap();
        let settings: Value = serde_json::from_str(&raw).unwrap();
        let config = DiscordConfig::from_settings(&settings).expect("no creds");
        println!(
            "client_id len: {}, token len: {}",
            config.client_id.len(),
            config.access_token.len()
        );
        let mut pipe = Pipe::open().unwrap();
        let deadline = Instant::now() + Duration::from_secs(4);
        pipe.write_all(
            &encode_frame(
                OP_HANDSHAKE,
                &json!({ "v": 1, "client_id": config.client_id }).to_string(),
            ),
        )
        .unwrap();
        let mut header = vec![0u8; 8];
        match pipe.read_exact(&mut header, deadline) {
            Ok(()) => {
                let len = i32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
                let mut rest = vec![0u8; len];
                pipe.read_exact(&mut rest, deadline).unwrap();
                let reply = String::from_utf8_lossy(&rest);
                println!("handshake reply evt: {}", {
                    let v: Value = serde_json::from_str(&reply).unwrap_or(Value::Null);
                    v["evt"].as_str().unwrap_or("?").to_string()
                });
                // now try AUTHENTICATE on the same connection
                pipe.write_all(
                    &encode_frame(
                        OP_FRAME,
                        &json!({
                            "cmd": "AUTHENTICATE",
                            "args": { "access_token": config.access_token },
                            "nonce": "probe-1",
                        })
                        .to_string(),
                    ),
                )
                .unwrap();
                let mut header2 = vec![0u8; 8];
                match pipe.read_exact(&mut header2, deadline) {
                    Ok(()) => {
                        let len2 =
                            i32::from_le_bytes(header2[4..8].try_into().unwrap()) as usize;
                        let mut rest2 = vec![0u8; len2];
                        pipe.read_exact(&mut rest2, deadline).unwrap();
                        let text = String::from_utf8_lossy(&rest2);
                        let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                        println!(
                            "auth reply: cmd={:?} evt={:?} nonce={:?} msg={:?}",
                            v["cmd"].as_str(),
                            v["evt"].as_str(),
                            v["nonce"].as_str(),
                            v["data"]["message"].as_str(),
                        );
                    }
                    Err(e) => println!("SILENCE after AUTHENTICATE: {e}"),
                }
            }
            Err(e) => println!("SILENCE after handshake: {e}"),
        }
    }
}
