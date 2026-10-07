//! Authorization Code + PKCE against `accounts.spotify.com`, plus the
//! ephemeral loopback listener that catches the browser redirect
//! (RFC 8252). The desktop host runs [`login`] inside `spawn_blocking`;
//! the headless server never logs in interactively (it just reads the
//! same `spotify.json`).

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use crate::config::SpotifyConfig;
use crate::error::{Result, SpotifyError};
use crate::http::{HttpRequest, Method, Transport};
use crate::pkce::{
    authorize_url, new_state, new_verifier, s256_challenge, CALLBACK_TIMEOUT, REDIRECT_URI,
};

const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
const ME_URL: &str = "https://api.spotify.com/v1/me";

/// Tokens issued by one exchange/refresh.
#[derive(Clone)]
pub struct Tokens {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Seconds the access token lives (typically 3600).
    pub expires_in: u64,
}

/// x-www-form-urlencoded body from pairs.
fn form(pairs: &[(&str, &str)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| format!("{}={}", k, crate::pkce::urlencode(v)))
        .collect::<Vec<_>>()
        .join("&")
}

fn token_request(transport: &dyn Transport, body: &str) -> Result<Tokens> {
    let req = HttpRequest {
        method: Method::Post,
        url: TOKEN_URL.into(),
        headers: vec![(
            "content-type".into(),
            "application/x-www-form-urlencoded".into(),
        )],
        body: Some(body.as_bytes().to_vec()),
    };
    let resp = transport
        .send(&req)
        .map_err(|_| SpotifyError::Network("token request"))?;
    let parsed = resp
        .json()
        .ok_or_else(|| SpotifyError::Login("token endpoint returned no JSON".into()))?;
    if resp.status == 400 {
        // invalid_grant / invalid_client: Spotify itself rejected it
        let error = parsed
            .pointer("/error")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if error == "invalid_grant" {
            return Err(SpotifyError::NeedsLogin);
        }
        return Err(SpotifyError::Login(format!(
            "token endpoint rejected the grant: {error}"
        )));
    }
    if !(200..300).contains(&resp.status) {
        return Err(SpotifyError::Login(format!(
            "token endpoint answered HTTP {}",
            resp.status
        )));
    }
    let access_token = parsed
        .pointer("/access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| SpotifyError::Login("token response has no access_token".into()))?
        .to_string();
    let expires_in = parsed
        .pointer("/expires_in")
        .and_then(|v| v.as_u64())
        .unwrap_or(3600);
    let refresh_token = parsed
        .pointer("/refresh_token")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Ok(Tokens {
        access_token,
        refresh_token,
        expires_in,
    })
}

/// Exchange the authorization code (PKCE verifier included, no secret).
pub fn exchange_code(
    transport: &dyn Transport,
    client_id: &str,
    code: &str,
    verifier: &str,
) -> Result<Tokens> {
    token_request(
        transport,
        &form(&[
            ("client_id", client_id),
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", REDIRECT_URI),
            ("code_verifier", verifier),
        ]),
    )
}

/// Silent refresh. A rotating response carries a new refresh token -
/// the caller MUST store it (Spotify reportedly invalidates the old one).
/// `invalid_grant` maps to [`SpotifyError::NeedsLogin`] and is never retried.
pub fn refresh_tokens(
    transport: &dyn Transport,
    client_id: &str,
    refresh_token: &str,
) -> Result<Tokens> {
    token_request(
        transport,
        &form(&[
            ("client_id", client_id),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ]),
    )
}

/// `/v1/me`: display name + product, for the settings status line and
/// the Free-account Premium warning.
pub fn fetch_me(
    transport: &dyn Transport,
    access_token: &str,
) -> Result<(Option<String>, Option<String>)> {
    let req = HttpRequest {
        method: Method::Get,
        url: ME_URL.into(),
        headers: vec![("authorization".into(), format!("Bearer {access_token}"))],
        body: None,
    };
    let resp = transport
        .send(&req)
        .map_err(|_| SpotifyError::Network("/v1/me"))?;
    if resp.status == 401 {
        return Err(SpotifyError::NeedsLogin);
    }
    if !(200..300).contains(&resp.status) {
        return Err(SpotifyError::Api(format!(
            "/v1/me answered HTTP {}",
            resp.status
        )));
    }
    let parsed = resp.json().unwrap_or_default();
    let name = parsed
        .pointer("/display_name")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let product = parsed
        .pointer("/product")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    Ok((name, product))
}

/// What one callback GET resolved to.
#[derive(Debug, PartialEq)]
pub enum Callback {
    /// Matching state + authorization code.
    Code(String),
    /// Spotify redirected with `error=` (user declined).
    Denied,
}

/// Percent-decode one query value (codes are URL-safe, but be strict).
fn decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                match u8::from_str_radix(hex, 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                    }
                    Err(_) => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Parse `a=b&c=d` into pairs (values decoded).
pub fn parse_query(query: &str) -> Vec<(String, String)> {
    query
        .split('&')
        .filter(|s| !s.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((k, v)) => (decode(k), decode(v)),
            None => (decode(pair), String::new()),
        })
        .collect()
}

/// The query string of a request line (`GET /p?a=b HTTP/1.1` -> `a=b`).
fn query_of(request_line: &str) -> &str {
    request_line
        .split_whitespace()
        .nth(1)
        .unwrap_or("")
        .split_once('?')
        .map(|(_, q)| q)
        .unwrap_or("")
}

/// Classify one callback request line. `GET /spotify/callback?...`.
fn classify_callback(request_line: &str) -> Option<Callback> {
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?;
    let target = parts.next()?;
    if method != "GET" {
        return None;
    }
    let path = target.split_once('?').map(|(p, _)| p).unwrap_or(target);
    if path != "/spotify/callback" {
        return None;
    }
    let params = parse_query(query_of(request_line));
    let get = |key: &str| {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };
    if let Some(error) = get("error") {
        if !error.is_empty() {
            return Some(Callback::Denied);
        }
    }
    get("code").filter(|c| !c.is_empty()).map(Callback::Code)
}

/// Read one HTTP request from the socket (headers are enough: the
/// callback is a bodiless GET). Sockets carry a read timeout so a
/// half-open connection cannot eat the whole login budget.
fn read_request(stream: &mut TcpStream) -> std::io::Result<Option<String>> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        let n = match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                break
            }
            Err(e) => return Err(e),
        };
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") || buf.len() > 8192 {
            break;
        }
    }
    if buf.is_empty() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&buf).into_owned()))
}

fn write_page(stream: &mut TcpStream, page: &str) {
    let bytes = page.as_bytes();
    let with_length = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/html; charset=utf-8\r\nconnection: close\r\ncontent-length: {}\r\n\r\n",
        bytes.len()
    );
    let _ = stream.write_all(with_length.as_bytes());
    let _ = stream.write_all(bytes);
    let _ = stream.flush();
}

/// Accept connections until one GET to `/spotify/callback` arrives with
/// the expected `state`. Probes and mismatched states get a short error
/// page and the listener keeps waiting (a hostile probe must not be able
/// to consume the login). Gives up after `timeout`.
pub fn wait_callback(
    listener: TcpListener,
    expected_state: &str,
    timeout: Duration,
) -> Result<String> {
    let deadline = Instant::now() + timeout;
    listener
        .set_nonblocking(false)
        .map_err(|e| SpotifyError::Login(format!("callback listener: {e}")))?;
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Err(SpotifyError::Login(
                "login timed out waiting for the browser redirect".into(),
            ));
        }
        // poll accept so the deadline is noticed even without traffic
        listener
            .set_nonblocking(true)
            .map_err(|e| SpotifyError::Login(format!("callback listener: {e}")))?;
        let accepted = listener.accept();
        listener
            .set_nonblocking(false)
            .map_err(|e| SpotifyError::Login(format!("callback listener: {e}")))?;
        let (mut stream, _) = match accepted {
            Ok(pair) => pair,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(100).min(deadline - now));
                continue;
            }
            Err(e) => return Err(SpotifyError::Login(format!("callback accept: {e}"))),
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let Some(request) = read_request(&mut stream).ok().flatten() else {
            continue;
        };
        let request_line = request.lines().next().unwrap_or_default().to_string();
        let params_ok = || {
            parse_query(query_of(&request_line))
                .iter()
                .any(|(k, v)| k == "state" && v == expected_state)
        };
        match classify_callback(&request_line) {
            Some(Callback::Code(code)) if params_ok() => {
                write_page(
                    &mut stream,
                    &callback_page_body(
                        "Pulpit \u{2014} Spotify",
                        "Login complete \u{2014} you can close this tab.",
                    ),
                );
                return Ok(code);
            }
            Some(Callback::Denied) => {
                write_page(
                    &mut stream,
                    &callback_page_body("Pulpit \u{2014} Spotify", "Login was declined."),
                );
                return Err(SpotifyError::Login("authorization was declined".into()));
            }
            _ => {
                // wrong path, wrong state, or a probe: answer and keep waiting
                write_page(
                    &mut stream,
                    &callback_page_body(
                        "Pulpit \u{2014} Spotify",
                        "Unexpected callback \u{2014} waiting for the login redirect.",
                    ),
                );
            }
        }
    }
}

fn callback_page_body(title: &str, body: &str) -> String {
    format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head>\
         <body style=\"background:#191414;color:#fff;font-family:system-ui,sans-serif;display:grid;place-items:center;height:100vh;margin:0\">\
         <div style=\"text-align:center\"><p style=\"font-size:42px;color:#1DB954\">\u{25CF}</p><p style=\"font-size:18px\">{body}</p></div></body></html>"
    )
}

/// Full interactive login. Blocking (network + browser wait) - the host
/// runs it in `spawn_blocking`. Steps (design §2):
///
/// 1. verifier + S256 challenge + random state,
/// 2. `open_browser(authorize_url)` - the host decides how to open it,
/// 3. ephemeral listener on 127.0.0.1:8502 catches exactly one
///    `/spotify/callback` GET (state-checked, 5 min budget),
/// 4. code exchange,
/// 5. `/v1/me` for the display name and product.
///
/// Returns the config to persist; the host then builds the [`crate::Spotify`]
/// handle with it. A busy 8502 is a hard error - the redirect URI is
/// registered exactly, so there is no port fallback.
pub fn login(client_id: &str, open_browser: impl FnOnce(&str)) -> Result<SpotifyConfig> {
    let verifier = new_verifier();
    let state = new_state();
    // bind BEFORE the browser opens: a busy 8502 must fail the login
    // with a clear error, not after the user has already signed in
    let listener = TcpListener::bind(("127.0.0.1", 8502)).map_err(|e| {
        SpotifyError::Login(format!(
            "cannot listen on 127.0.0.1:8502 for the Spotify callback ({e}) - is another login in progress?"
        ))
    })?;
    let url = authorize_url(client_id, &s256_challenge(&verifier), &state);
    open_browser(&url);

    let code = wait_callback(listener, &state, CALLBACK_TIMEOUT)?;

    let transport = crate::http::UreqTransport::new();
    let tokens = exchange_code(&transport, client_id, &code, &verifier)?;
    let (user, product) = fetch_me(&transport, &tokens.access_token)?;
    Ok(SpotifyConfig {
        client_id: client_id.to_string(),
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token,
        expires_at: Some(now_unix() + tokens.expires_in as i64),
        user,
        product,
    })
}

/// Wall-clock unix seconds.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::HttpResponse;

    fn token_ok(refresh: Option<&str>) -> HttpResponse {
        let mut body =
            r#"{"access_token":"NEW-ACCESS","token_type":"Bearer","expires_in":3600}"#.to_string();
        if let Some(r) = refresh {
            body = format!(
                r#"{{"access_token":"NEW-ACCESS","token_type":"Bearer","expires_in":3600,"refresh_token":"{r}"}}"#
            );
        }
        HttpResponse {
            status: 200,
            headers: Vec::new(),
            body: body.into_bytes(),
        }
    }

    #[test]
    fn exchange_sends_the_pkce_form_and_parses_tokens() {
        let fake = crate::http::FakeTransport::new();
        fake.push(token_ok(None));
        let tokens = exchange_code(&fake, "cid", "the-code", "the-verifier").unwrap();
        assert_eq!(tokens.access_token, "NEW-ACCESS");
        assert_eq!(tokens.refresh_token, None);
        assert_eq!(tokens.expires_in, 3600);
        let req = &fake.requests()[0];
        assert_eq!(req.url, "https://accounts.spotify.com/api/token");
        let body = String::from_utf8(req.body.clone().unwrap()).unwrap();
        assert!(body.contains("grant_type=authorization_code"));
        assert!(body.contains("code=the-code"));
        assert!(body.contains("code_verifier=the-verifier"));
        assert!(body.contains(&format!(
            "redirect_uri={}",
            crate::pkce::urlencode(REDIRECT_URI)
        )));
    }

    #[test]
    fn invalid_grant_maps_to_needs_login() {
        let fake = crate::http::FakeTransport::new();
        fake.push_json(400, serde_json::json!({"error": "invalid_grant"}));
        assert!(matches!(
            refresh_tokens(&fake, "cid", "stale"),
            Err(SpotifyError::NeedsLogin)
        ));
    }

    #[test]
    fn rotation_carries_the_new_refresh_token() {
        let fake = crate::http::FakeTransport::new();
        fake.push(token_ok(Some("ROTATED")));
        let tokens = refresh_tokens(&fake, "cid", "old").unwrap();
        assert_eq!(tokens.refresh_token.as_deref(), Some("ROTATED"));
    }

    #[test]
    fn me_parses_name_and_product() {
        let fake = crate::http::FakeTransport::new();
        fake.push_json(
            200,
            serde_json::json!({"display_name": "Tymek", "product": "premium", "id": "x"}),
        );
        let (user, product) = fetch_me(&fake, "tok").unwrap();
        assert_eq!(user.as_deref(), Some("Tymek"));
        assert_eq!(product.as_deref(), Some("premium"));
    }

    #[test]
    fn callback_classification() {
        assert_eq!(
            classify_callback("GET /spotify/callback?code=AQC..&state=st HTTP/1.1"),
            Some(Callback::Code("AQC..".into()))
        );
        assert_eq!(
            classify_callback("GET /spotify/callback?error=access_denied&state=st HTTP/1.1"),
            Some(Callback::Denied)
        );
        assert_eq!(classify_callback("GET /favicon.ico HTTP/1.1"), None);
        assert_eq!(
            classify_callback("GET /spotify/callback?code=&state=st HTTP/1.1"),
            None
        );
    }

    #[test]
    fn query_values_decode() {
        let params = parse_query("code=abc&state=x%20y&empty=");
        assert_eq!(params[0], ("code".into(), "abc".into()));
        assert_eq!(params[1], ("state".into(), "x y".into()));
        assert_eq!(params[2], ("empty".into(), "".into()));
    }

    /// In-process loopback: the listener on an ephemeral port answers a
    /// scripted browser GET. Loopback-only, like the discord crate's
    /// network-refused test.
    #[test]
    fn callback_listener_accepts_matching_state_and_rejects_probes() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let waiter = std::thread::spawn(move || {
            wait_callback(listener, "expected", Duration::from_secs(10))
        });

        // a probe with the wrong state is answered but does not finish the login
        let mut probe = TcpStream::connect(addr).unwrap();
        probe
            .write_all(b"GET /spotify/callback?code=EVIL&state=wrong HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut page = String::new();
        probe.read_to_string(&mut page).unwrap();
        assert!(page.contains("Unexpected callback"));

        // the real redirect
        let mut browser = TcpStream::connect(addr).unwrap();
        browser
            .write_all(
                b"GET /spotify/callback?code=AQCreal&state=expected HTTP/1.1\r\nHost: x\r\n\r\n",
            )
            .unwrap();
        let mut page = String::new();
        browser.read_to_string(&mut page).unwrap();
        assert!(page.contains("close this tab"));

        assert_eq!(waiter.join().unwrap().unwrap(), "AQCreal");
    }

    #[test]
    fn declined_callback_fails_the_login() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let addr = listener.local_addr().unwrap();
        let waiter = std::thread::spawn(move || {
            wait_callback(listener, "expected", Duration::from_secs(10))
        });
        let mut browser = TcpStream::connect(addr).unwrap();
        browser
            .write_all(b"GET /spotify/callback?error=access_denied&state=expected HTTP/1.1\r\nHost: x\r\n\r\n")
            .unwrap();
        let mut page = String::new();
        browser.read_to_string(&mut page).unwrap();
        assert!(page.contains("declined"));
        assert!(matches!(
            waiter.join().unwrap(),
            Err(SpotifyError::Login(_))
        ));
    }
}
