//! OBS configuration file (`obs.json`).

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObsConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub password: Option<String>,
}

fn default_host() -> String {
    "127.0.0.1".to_string()
}

fn default_port() -> u16 {
    4455
}

impl Default for ObsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_host(),
            port: default_port(),
            password: None,
        }
    }
}

impl std::fmt::Debug for ObsConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObsConfig")
            .field("enabled", &self.enabled)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("password", &self.password.as_ref().map(|_| "*********"))
            .finish()
    }
}

impl ObsConfig {
    pub fn load(path: &Path) -> Result<Self, String> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes =
            std::fs::read(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        serde_json::from_slice(&bytes).map_err(|e| format!("cannot parse {}: {e}", path.display()))
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        pulpit_db::write_atomic(path, &json).map_err(|e| e.to_string())
    }

    pub fn ws_url(&self) -> String {
        format!("ws://{}:{}", self.host, self.port)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_save_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("obs.json");
        let cfg = ObsConfig {
            enabled: true,
            host: "192.168.1.50".to_string(),
            port: 4455,
            password: Some("mypass".to_string()),
        };
        cfg.save(&path).unwrap();
        let loaded = ObsConfig::load(&path).unwrap();
        assert_eq!(cfg, loaded);
        let dbg = format!("{cfg:?}");
        assert!(!dbg.contains("mypass"));
        assert!(dbg.contains("********"));
    }
}
