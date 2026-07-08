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
}

impl Default for Config {
    fn default() -> Self {
        Self {
            host_port: 21118,
            require_manual_approval: true,
            session_code_ttl_minutes: 10,
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
            ..Config::default()
        };
        cfg.save(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), cfg);
    }
}
