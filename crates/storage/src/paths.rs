use std::path::{Path, PathBuf};

use directories::ProjectDirs;

use crate::StorageError;

/// Resolves and creates the platform-standard directories for config and data.
#[derive(Debug, Clone)]
pub struct AppPaths {
    config_dir: PathBuf,
    data_dir: PathBuf,
}

impl AppPaths {
    /// Discovers the standard directories (e.g. `~/.config/controlis` and
    /// `~/.local/share/controlis` on Linux) and ensures they exist.
    pub fn discover() -> Result<Self, StorageError> {
        let dirs = ProjectDirs::from("dev", "controlis", "controlis")
            .ok_or(StorageError::NoDataDir)?;
        let config_dir = dirs.config_dir().to_path_buf();
        let data_dir = dirs.data_dir().to_path_buf();
        std::fs::create_dir_all(&config_dir)?;
        std::fs::create_dir_all(&data_dir)?;
        Ok(Self { config_dir, data_dir })
    }

    /// Uses an explicit base directory (used by tests to avoid touching the user's
    /// real config).
    pub fn with_base(base: &Path) -> Result<Self, StorageError> {
        let config_dir = base.join("config");
        let data_dir = base.join("data");
        std::fs::create_dir_all(&config_dir)?;
        std::fs::create_dir_all(&data_dir)?;
        Ok(Self { config_dir, data_dir })
    }

    pub fn config_file(&self) -> PathBuf {
        self.config_dir.join("config.toml")
    }

    pub fn database_file(&self) -> PathBuf {
        self.data_dir.join("controlis.db")
    }

    pub fn certificate_file(&self) -> PathBuf {
        self.data_dir.join("host_cert.der")
    }

    pub fn private_key_file(&self) -> PathBuf {
        self.data_dir.join("host_key.der")
    }

    pub fn iroh_secret_key_file(&self) -> PathBuf {
        self.data_dir.join("iroh_secret_key.txt")
    }
}
