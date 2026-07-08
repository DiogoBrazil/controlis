//! Local persistence: a TOML config file plus a SQLite database for connection
//! logs and known peers (the basis of trust-on-first-use).
//!
//! Secrets are never stored here in cleartext: session codes live only in memory;
//! only certificate fingerprints (already public) and audit metadata are persisted.

mod config;
mod db;
mod paths;

pub use config::Config;
pub use db::{ConnectionLog, KnownPeer, Store};
pub use paths::AppPaths;

/// Errors from configuration or database access.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("could not determine a platform data directory")]
    NoDataDir,
    #[error("i/o error: {0}")]
    Io(#[from] std::io::Error),
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("failed to parse config: {0}")]
    ConfigParse(#[from] toml::de::Error),
    #[error("failed to serialize config: {0}")]
    ConfigSerialize(#[from] toml::ser::Error),
}
