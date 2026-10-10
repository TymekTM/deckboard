//! Authentication for obs-websocket v5.
//!
//! Protocol formula:
//! `base64(sha256(base64(sha256(password + salt)) + challenge))`

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use sha2::{Digest, Sha256};

/// Compute the obs-websocket v5 authentication response string.
pub fn compute_auth_response(password: &str, salt: &str, challenge: &str) -> String {
    let mut secret_hasher = Sha256::new();
    secret_hasher.update(password.as_bytes());
    secret_hasher.update(salt.as_bytes());
    let secret_hash = secret_hasher.finalize();
    let secret_b64 = BASE64.encode(secret_hash);

    let mut auth_hasher = Sha256::new();
    auth_hasher.update(secret_b64.as_bytes());
    auth_hasher.update(challenge.as_bytes());
    let auth_hash = auth_hasher.finalize();
    BASE64.encode(auth_hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_auth_vector() {
        let pw = "supersecret";
        let salt = "dDJvSnptZ3ZZS2lQ";
        let challenge = "eUptM01KcUdNYUZj";
        let resp = compute_auth_response(pw, salt, challenge);
        assert_eq!(resp, "Bk6GwWMFwJCXNyNPWSnqaUgrlgkR3i5zNbKXRF7DhbA=");
    }
}
