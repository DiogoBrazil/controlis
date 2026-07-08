use std::path::Path;

use rusqlite::Connection;

use crate::StorageError;

/// A recorded connection attempt, for the local audit log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionLog {
    pub peer_addr: String,
    pub peer_fingerprint: Option<String>,
    pub accepted: bool,
    pub detail: String,
}

/// A peer whose certificate fingerprint has been seen before (trust-on-first-use).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownPeer {
    pub fingerprint: String,
    pub nickname: String,
}

/// SQLite-backed store for logs and known peers.
#[derive(Debug)]
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens (creating if needed) the database at `path` and applies the schema.
    pub fn open(path: &Path) -> Result<Self, StorageError> {
        let conn = Connection::open(path)?;
        Self::from_connection(conn)
    }

    /// Opens an in-memory store (used by tests).
    pub fn in_memory() -> Result<Self, StorageError> {
        let conn = Connection::open_in_memory()?;
        Self::from_connection(conn)
    }

    fn from_connection(conn: Connection) -> Result<Self, StorageError> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS connection_log (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 ts INTEGER NOT NULL DEFAULT (strftime('%s','now')),
                 peer_addr TEXT NOT NULL,
                 peer_fingerprint TEXT,
                 accepted INTEGER NOT NULL,
                 detail TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS known_peer (
                 fingerprint TEXT PRIMARY KEY,
                 nickname TEXT NOT NULL,
                 first_seen INTEGER NOT NULL DEFAULT (strftime('%s','now'))
             );",
        )?;
        Ok(Self { conn })
    }

    /// Appends a connection attempt to the audit log.
    pub fn record_connection(&self, log: &ConnectionLog) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO connection_log (peer_addr, peer_fingerprint, accepted, detail)
             VALUES (?1, ?2, ?3, ?4)",
            rusqlite::params![
                log.peer_addr,
                log.peer_fingerprint,
                log.accepted as i64,
                log.detail,
            ],
        )?;
        Ok(())
    }

    /// Returns the most recent `limit` connection log entries, newest first.
    pub fn recent_connections(&self, limit: u32) -> Result<Vec<ConnectionLog>, StorageError> {
        let mut stmt = self.conn.prepare(
            "SELECT peer_addr, peer_fingerprint, accepted, detail
             FROM connection_log ORDER BY id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |row| {
            Ok(ConnectionLog {
                peer_addr: row.get(0)?,
                peer_fingerprint: row.get(1)?,
                accepted: row.get::<_, i64>(2)? != 0,
                detail: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<Result<_, _>>()?)
    }

    /// Looks up a known peer by certificate fingerprint.
    pub fn known_peer(&self, fingerprint: &str) -> Result<Option<KnownPeer>, StorageError> {
        let mut stmt = self
            .conn
            .prepare("SELECT fingerprint, nickname FROM known_peer WHERE fingerprint = ?1")?;
        let mut rows = stmt.query_map([fingerprint], |row| {
            Ok(KnownPeer {
                fingerprint: row.get(0)?,
                nickname: row.get(1)?,
            })
        })?;
        match rows.next() {
            Some(peer) => Ok(Some(peer?)),
            None => Ok(None),
        }
    }

    /// Inserts or updates a known peer.
    pub fn remember_peer(&self, peer: &KnownPeer) -> Result<(), StorageError> {
        self.conn.execute(
            "INSERT INTO known_peer (fingerprint, nickname) VALUES (?1, ?2)
             ON CONFLICT(fingerprint) DO UPDATE SET nickname = excluded.nickname",
            rusqlite::params![peer.fingerprint, peer.nickname],
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn logs_are_returned_newest_first() {
        let store = Store::in_memory().unwrap();
        for i in 0..3 {
            store
                .record_connection(&ConnectionLog {
                    peer_addr: format!("10.0.0.{i}"),
                    peer_fingerprint: None,
                    accepted: i % 2 == 0,
                    detail: "test".into(),
                })
                .unwrap();
        }
        let recent = store.recent_connections(10).unwrap();
        assert_eq!(recent.len(), 3);
        assert_eq!(recent[0].peer_addr, "10.0.0.2");
    }

    #[test]
    fn known_peer_upsert_and_lookup() {
        let store = Store::in_memory().unwrap();
        assert!(store.known_peer("ab:cd").unwrap().is_none());
        store
            .remember_peer(&KnownPeer {
                fingerprint: "ab:cd".into(),
                nickname: "laptop".into(),
            })
            .unwrap();
        let found = store.known_peer("ab:cd").unwrap().unwrap();
        assert_eq!(found.nickname, "laptop");

        store
            .remember_peer(&KnownPeer {
                fingerprint: "ab:cd".into(),
                nickname: "renamed".into(),
            })
            .unwrap();
        assert_eq!(store.known_peer("ab:cd").unwrap().unwrap().nickname, "renamed");
    }
}
