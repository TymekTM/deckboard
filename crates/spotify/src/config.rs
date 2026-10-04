//! `spotify.json`: client id + OAuth tokens + the account facts the
//! settings UI shows. Written with `pulpit_db::write_atomic`; the
//! `Debug` impl redacts every token (logs are kept 14 days, the discord
//! crate's discipline).

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::error::{Result, SpotifyError};

/// Configuration persisted at `<data dir>/spotify.json`.
#[derive(Clone, Default, Serialize, Deserialize)]
pub struct SpotifyConfig {
    /// The user's own Spotify app id (bring-your-own, design §2).
    pub client_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Unix seconds when the access token expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<i64>,
    /// `display_name` from `/v1/me`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    /// `product` from `/v1/me` ("premium" / "free").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<String>,
}

impl std::fmt::Debug for SpotifyConfig {
    /// Tokens must never reach logs: only the public client id and the
    /// account facts print.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SpotifyConfig")
            .field("client_id", &self.client_id)
            .field("access_token", &"<redacted>")
            .field("refresh_token", &self.refresh_token.as_ref().map(|_| "<redacted>"))
            .field("expires_at", &self.expires_at)
            .field("user", &self.user)
            .field("product", &self.product)
            .finish()
    }
}

impl SpotifyConfig {
    /// Read the config. A missing file is a default (Spotify disabled);
    /// a file that exists but cannot be parsed errors WITHOUT touching
    /// anything, mirroring the discord crate's settings discipline.
    pub fn load(path: &Path) -> Result<SpotifyConfig> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text).map_err(|e| {
                SpotifyError::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("spotify.json exists but is not valid JSON: {e}"),
                ))
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(SpotifyConfig::default()),
            Err(e) => Err(SpotifyError::Io(e)),
        }
    }

    /// Persist atomically (`<path>.tmp` + rename, never a torn file).
    pub fn save(&self, path: &Path) -> Result<()> {
        let json = serde_json::to_string_pretty(self).map_err(|e| {
            SpotifyError::Io(std::io::Error::other(format!("spotify.json serialization: {e}")))
        })?;
        Ok(pulpit_db::write_atomic(path, json.as_bytes())?)
    }

    /// True when a refresh token exists: login has happened at least
    /// once (an access token alone is useless across restarts).
    pub fn has_login(&self) -> bool {
        self.refresh_token.as_deref().is_some_and(|t| !t.is_empty())
    }
}

/// Logout: delete the tokens (and account facts derived from them) but
/// keep the client id, so the next login does not need re-pasting
/// (design §2).
pub fn logout(path: &Path) -> Result<()> {
    let mut config = SpotifyConfig::load(path)?;
    config.access_token = String::new();
    config.refresh_token = None;
    config.expires_at = None;
    config.user = None;
    config.product = None;
    config.save(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spotify.json");
        let config = SpotifyConfig::load(&path).unwrap();
        assert_eq!(config.client_id, "");
        assert!(!config.has_login());

        let config = SpotifyConfig {
            client_id: "cid".into(),
            access_token: "at".into(),
            refresh_token: Some("rt".into()),
            expires_at: Some(1_800_000_000),
            user: Some("Tymek".into()),
            product: Some("premium".into()),
        };
        config.save(&path).unwrap();
        let back = SpotifyConfig::load(&path).unwrap();
        assert_eq!(back.client_id, "cid");
        assert_eq!(back.refresh_token.as_deref(), Some("rt"));
        assert!(back.has_login());
    }

    #[test]
    fn corrupt_file_errors_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spotify.json");
        std::fs::write(&path, "not json").unwrap();
        assert!(SpotifyConfig::load(&path).is_err());
    }

    #[test]
    fn debug_redacts_tokens() {
        let config = SpotifyConfig {
            client_id: "cid".into(),
            access_token: "SECRET-ACCESS".into(),
            refresh_token: Some("SECRET-REFRESH".into()),
            expires_at: Some(1),
            user: None,
            product: None,
        };
        let text = format!("{config:?}");
        assert!(text.contains("client_id"));
        assert!(!text.contains("SECRET-ACCESS"));
        assert!(!text.contains("SECRET-REFRESH"));
        assert!(text.contains("<redacted>"));
    }

    #[test]
    fn logout_keeps_the_client_id() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("spotify.json");
        SpotifyConfig {
            client_id: "cid".into(),
            access_token: "at".into(),
            refresh_token: Some("rt".into()),
            expires_at: Some(1),
            user: Some("u".into()),
            product: Some("free".into()),
        }
        .save(&path)
        .unwrap();
        logout(&path).unwrap();
        let config = SpotifyConfig::load(&path).unwrap();
        assert_eq!(config.client_id, "cid");
        assert_eq!(config.access_token, "");
        assert_eq!(config.refresh_token, None);
        assert_eq!(config.user, None);
    }
}
