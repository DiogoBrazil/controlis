use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::StorageError;

/// User-editable application settings, persisted as TOML.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// UDP port the host listens on.
    pub host_port: u16,
    /// Require the host user to approve each incoming connection.
    pub require_manual_approval: bool,
    /// Minutes before an unused session code expires.
    pub session_code_ttl_minutes: u64,
    /// IPv4 embedded in the host's access code. Leave unset to auto-detect the
    /// LAN address; set it explicitly on machines with several interfaces
    /// (VPNs, virtual adapters) when detection picks the wrong one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub advertised_ip: Option<std::net::Ipv4Addr>,
    /// Rendezvous server base URL used for internet sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendezvous_url: Option<String>,
    /// Self-hosted Iroh relay URL used for internet sessions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay_url: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host_port: 21118,
            require_manual_approval: true,
            session_code_ttl_minutes: 10,
            advertised_ip: None,
            rendezvous_url: None,
            relay_url: None,
        }
    }
}

impl Config {
    /// Loads config from `path`, returning defaults if the file does not exist.
    pub fn load(path: &Path) -> Result<Self, StorageError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(toml::from_str(&text)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Writes the config to `path` as TOML.
    pub fn save(&self, path: &Path) -> Result<(), StorageError> {
        let text = toml::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_missing_file_yields_defaults() {
        let dir = std::env::temp_dir().join(format!("controlis-cfg-{}", std::process::id()));
        let path = dir.join("nope.toml");
        assert_eq!(Config::load(&path).unwrap(), Config::default());
    }

    #[test]
    fn save_then_load_roundtrips() {
        let dir = std::env::temp_dir().join(format!("controlis-cfg-rt-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        let cfg = Config {
            host_port: 30000,
            require_manual_approval: false,
            advertised_ip: Some(std::net::Ipv4Addr::new(192, 168, 0, 10)),
            rendezvous_url: Some("https://controlis.example.test".into()),
            relay_url: Some("https://relay.example.test".into()),
            ..Config::default()
        };
        cfg.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), cfg);
    }
}
