//! PKCE (RFC 7636, S256) helpers and the authorize URL. Pure functions:
//! verifier/challenge generation is fully unit-testable against the
//! RFC appendix B vector.

use rand::Rng;

/// Characters allowed in a verifier (RFC 7636 §4.1: unreserved ASCII).
const VERIFIER_ALPHABET: &[u8] =
    b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-._~";

/// The redirect URI the user must register in the Spotify dashboard.
/// The port is fixed and independent of `PULPIT_PORT`: the URI has to
/// match the registration exactly (design §2), so a busy 8502 is a hard
/// login error, never a fallback to another port.
pub const REDIRECT_URI: &str = "http://127.0.0.1:8502/spotify/callback";

/// How long the ephemeral callback listener waits for the browser
/// redirect before giving up (design §2: 5 min).
pub const CALLBACK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5 * 60);

/// Scopes requested at authorization (design §2).
pub const SCOPES: &str = "user-read-playback-state user-read-currently-playing user-modify-playback-state user-library-read user-library-modify playlist-read-private playlist-read-collaborative";

/// Random PKCE verifier: 64 chars from the unreserved set (RFC allows
/// 43..=128; 64 is comfortably inside).
pub fn new_verifier() -> String {
    random_string(64)
}

/// Random OAuth `state`, long enough to be unguessable.
pub fn new_state() -> String {
    random_string(24)
}

fn random_string(len: usize) -> String {
    let mut rng = rand::thread_rng();
    (0..len)
        .map(|_| VERIFIER_ALPHABET[rng.gen_range(0..VERIFIER_ALPHABET.len())] as char)
        .collect()
}

/// S256 code challenge: BASE64URL-ENCODE(SHA256(ASCII(verifier))),
/// padding stripped (RFC 7636 §4.2).
pub fn s256_challenge(verifier: &str) -> String {
    use base64::Engine as _;
    use sha2::Digest as _;
    let digest = sha2::Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

/// `https://accounts.spotify.com/authorize?...` with PKCE + state.
pub fn authorize_url(client_id: &str, challenge: &str, state: &str) -> String {
    format!(
        "https://accounts.spotify.com/authorize?client_id={}&response_type=code&redirect_uri={}&code_challenge_method=S256&code_challenge={}&scope={}&state={}",
        urlencode(client_id),
        urlencode(REDIRECT_URI),
        urlencode(challenge),
        urlencode(SCOPES),
        urlencode(state),
    )
}

/// Percent-encoding for query values (same character set as the discord
/// crate's helper).
pub fn urlencode(s: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// RFC 7636 appendix B test vector.
    #[test]
    fn s256_matches_rfc7636_appendix_b() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(
            s256_challenge(verifier),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }

    #[test]
    fn verifier_and_state_use_only_unreserved_chars() {
        for s in [new_verifier(), new_state()] {
            assert_eq!(s.len(), s.chars().count());
            assert!(s.chars().all(|c| VERIFIER_ALPHABET.contains(&(c as u8))));
        }
        assert_eq!(new_verifier().len(), 64);
    }

    #[test]
    fn authorize_url_carries_pkce_and_fixed_redirect() {
        let url = authorize_url("client id+", "challenge", "st");
        assert!(url.starts_with("https://accounts.spotify.com/authorize?"));
        assert!(url.contains("client_id=client%20id%2B"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains(&format!("redirect_uri={}", urlencode(REDIRECT_URI))));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("code_challenge=challenge"));
        assert!(url.contains("state=st"));
        assert!(url.contains("scope=user-read-playback-state"));
    }
}
