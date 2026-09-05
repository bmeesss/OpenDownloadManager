//! SQLite persistence for downloads.
//!
//! SQLite is the only persistence layer. The schema is versioned through
//! explicit migrations so upgrades are reproducible. The generic `downloads`
//! table stores only protocol-neutral columns plus an opaque `backend_meta`
//! JSON blob for protocol-specific data, so a future BitTorrent backend fits
//! without new generic columns.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use odm_core::{BackendKind, DownloadId, DownloadState, Error, Result};
use rusqlite::{params, types::Type, Connection};
use serde_json::Value;
use url::Url;

/// A single managed download, as held in memory and persisted to SQLite.
#[derive(Debug, Clone)]
pub struct Download {
    /// Stable identifier.
    pub id: DownloadId,
    /// Source URL.
    pub url: Url,
    /// Final on-disk destination.
    pub destination: std::path::PathBuf,
    /// Which backend executes the transfer.
    pub backend: BackendKind,
    /// Current lifecycle state.
    pub state: DownloadState,
    /// Whether an existing destination may be overwritten.
    pub overwrite: bool,
    /// Total expected bytes, if known.
    pub total_bytes: Option<u64>,
    /// Bytes transferred so far.
    pub downloaded_bytes: u64,
    /// When the download was created.
    pub created_at: SystemTime,
    /// When the download was last modified.
    pub updated_at: SystemTime,
    /// When the transfer started, if it has.
    pub started_at: Option<SystemTime>,
    /// When the transfer finished (success or terminal failure), if it has.
    pub completed_at: Option<SystemTime>,
    /// Last error description, if the download is in a terminal error state.
    pub error: Option<String>,
    /// Opaque, protocol-specific metadata (resume info, info hash, ...).
    pub backend_meta: Value,
}

/// Schema migrations, in order. Each is applied once inside its own
/// transaction and recorded in `schema_migrations`.
const MIGRATIONS: &[(i64, &str)] = &[
    (
        1,
        "CREATE TABLE IF NOT EXISTS downloads (
            id TEXT PRIMARY KEY,
            url TEXT NOT NULL,
            destination TEXT NOT NULL,
            backend TEXT NOT NULL,
            state TEXT NOT NULL,
            overwrite INTEGER NOT NULL DEFAULT 0,
            total_bytes INTEGER,
            downloaded_bytes INTEGER NOT NULL DEFAULT 0,
            error TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL,
            started_at INTEGER,
            completed_at INTEGER,
            backend_meta TEXT
        );",
    ),
    (
        2,
        "CREATE INDEX IF NOT EXISTS idx_downloads_state ON downloads(state);",
    ),
];

/// SQLite-backed persistence for [`Download`] records.
pub struct Persistence {
    conn: Mutex<Connection>,
}

impl Persistence {
    /// Opens (creating if needed) the database at `path` and applies pending
    /// migrations.
    ///
    /// # Errors
    /// Returns an [`Error::Internal`] if the file cannot be opened or a
    /// migration fails.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(db_err)?;
        let p = Self {
            conn: Mutex::new(conn),
        };
        p.migrate()?;
        Ok(p)
    }

    /// Inserts or replaces a download row.
    ///
    /// # Errors
    /// Returns an [`Error::Internal`] on a database failure.
    pub fn save(&self, dl: &Download) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO downloads \
             (id, url, destination, backend, state, overwrite, total_bytes, downloaded_bytes, \
              error, created_at, updated_at, started_at, completed_at, backend_meta) \
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14)",
            params![
                dl.id.to_string(),
                dl.url.to_string(),
                dl.destination.to_string_lossy().to_string(),
                dl.backend.to_string(),
                dl.state.to_string(),
                dl.overwrite as i64,
                dl.total_bytes.and_then(|v| i64::try_from(v).ok()),
                i64::try_from(dl.downloaded_bytes).unwrap_or(i64::MAX),
                dl.error.clone(),
                sys_to_ms(dl.created_at),
                sys_to_ms(dl.updated_at),
                dl.started_at.map(sys_to_ms),
                dl.completed_at.map(sys_to_ms),
                serde_json::to_string(&dl.backend_meta).unwrap_or_else(|_| "null".to_string()),
            ],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// List all persisted downloads.
    ///
    /// # Errors
    /// Returns an [`Error::Internal`] on a database failure.
    pub fn list(&self) -> Result<Vec<Download>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, url, destination, backend, state, overwrite, total_bytes, downloaded_bytes, \
                 error, created_at, updated_at, started_at, completed_at, backend_meta \
                 FROM downloads ORDER BY created_at ASC, id ASC",
            )
            .map_err(db_err)?;
        let rows = stmt.query_map((), row_to_download).map_err(db_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(db_err)?);
        }
        Ok(out)
    }

    /// Deletes a download row.
    ///
    /// # Errors
    /// Returns an [`Error::Internal`] on a database failure.
    pub fn remove(&self, id: DownloadId) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute(
            "DELETE FROM downloads WHERE id = ?1",
            params![id.to_string()],
        )
        .map_err(db_err)?;
        Ok(())
    }

    /// Returns the applied migration versions, ascending.
    ///
    /// # Errors
    /// Returns an [`Error::Internal`] on a database failure.
    pub fn applied_migrations(&self) -> Result<Vec<i64>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT version FROM schema_migrations ORDER BY version ASC")
            .map_err(db_err)?;
        let rows = stmt
            .query_map((), |row| row.get::<usize, i64>(0))
            .map_err(db_err)?;
        let mut out = Vec::new();
        for r in rows {
            out.push(r.map_err(db_err)?);
        }
        Ok(out)
    }

    fn migrate(&self) -> Result<()> {
        {
            let conn = self.conn.lock().unwrap();
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations \
                 (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL);",
            )
            .map_err(db_err)?;
        }
        let applied = self.applied_migrations()?;
        for &(version, sql) in MIGRATIONS {
            if applied.contains(&version) {
                continue;
            }
            let mut conn = self.conn.lock().unwrap();
            let tx = conn.transaction().map_err(db_err)?;
            tx.execute_batch(sql).map_err(db_err)?;
            tx.execute(
                "INSERT INTO schema_migrations (version, applied_at) VALUES (?1, ?2)",
                params![version, sys_to_ms(SystemTime::now()).to_string()],
            )
            .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
        }
        Ok(())
    }
}

fn sys_to_ms(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn ms_to_sys(ms: i64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(u64::try_from(ms.max(0)).unwrap_or(0))
}

fn db_err(e: rusqlite::Error) -> Error {
    Error::Internal(format!("database error: {e}"))
}

fn convert_err<E: core::error::Error + Send + Sync + 'static>(
    idx: usize,
    kind: Type,
    e: E,
) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(idx, kind, Box::new(e))
}

fn row_to_download(row: &rusqlite::Row<'_>) -> rusqlite::Result<Download> {
    use uuid::Uuid;

    let id_s: String = row.get(0)?;
    let url_s: String = row.get(1)?;
    let dest_s: String = row.get(2)?;
    let backend_s: String = row.get(3)?;
    let state_s: String = row.get(4)?;
    let overwrite: i64 = row.get(5)?;
    let total: Option<i64> = row.get(6)?;
    let downloaded: i64 = row.get(7)?;
    let error: Option<String> = row.get(8)?;
    let created: i64 = row.get(9)?;
    let updated: i64 = row.get(10)?;
    let started: Option<i64> = row.get(11)?;
    let completed: Option<i64> = row.get(12)?;
    let meta_s: Option<String> = row.get(13)?;

    let id = Uuid::parse_str(&id_s)
        .map(DownloadId)
        .map_err(|e| convert_err(0, Type::Text, e))?;
    let url = Url::parse(&url_s).map_err(|e| convert_err(1, Type::Text, e))?;
    let backend = backend_s
        .parse::<BackendKind>()
        .map_err(|e| convert_err(3, Type::Text, e))?;
    let state = state_s
        .parse::<DownloadState>()
        .map_err(|e| convert_err(4, Type::Text, e))?;
    let backend_meta = meta_s
        .as_deref()
        .and_then(|s| serde_json::from_str::<Value>(s).ok())
        .unwrap_or(Value::Null);

    Ok(Download {
        id,
        url,
        destination: std::path::PathBuf::from(dest_s),
        backend,
        state,
        overwrite: overwrite != 0,
        total_bytes: total.map(|v| u64::try_from(v).unwrap_or(0)),
        downloaded_bytes: u64::try_from(downloaded).unwrap_or(0),
        created_at: ms_to_sys(created),
        updated_at: ms_to_sys(updated),
        started_at: started.map(ms_to_sys),
        completed_at: completed.map(ms_to_sys),
        error,
        backend_meta,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn tmp() -> (TempDir, std::path::PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("db.sqlite");
        (dir, path)
    }

    fn sample(state: DownloadState) -> Download {
        Download {
            id: DownloadId::new(),
            url: Url::parse("http://a.test/x").unwrap(),
            destination: std::path::PathBuf::from("x.bin"),
            backend: BackendKind::Http,
            state,
            overwrite: false,
            total_bytes: Some(100),
            downloaded_bytes: 10,
            created_at: SystemTime::UNIX_EPOCH,
            updated_at: SystemTime::UNIX_EPOCH,
            started_at: None,
            completed_at: None,
            error: None,
            backend_meta: serde_json::json!({"k": "v"}),
        }
    }

    #[test]
    fn save_and_load_round_trips() {
        let (_dir, path) = tmp();
        let p = Persistence::open(&path).unwrap();
        let d = sample(DownloadState::Queued);
        p.save(&d).unwrap();
        let got = p
            .list()
            .unwrap()
            .into_iter()
            .find(|x| x.id == d.id)
            .unwrap();
        assert_eq!(got.id, d.id);
        assert_eq!(got.url, d.url);
        assert_eq!(got.state, DownloadState::Queued);
        assert_eq!(got.total_bytes, Some(100));
        assert_eq!(got.downloaded_bytes, 10);
        assert_eq!(got.backend_meta, serde_json::json!({"k": "v"}));
    }

    #[test]
    fn migrations_are_applied_once_and_idempotent() {
        let (_dir, path) = tmp();
        let p = Persistence::open(&path).unwrap();
        assert_eq!(p.applied_migrations().unwrap(), vec![1, 2]);
        drop(p);
        let p2 = Persistence::open(&path).unwrap();
        assert_eq!(p2.applied_migrations().unwrap(), vec![1, 2]);
    }

    #[test]
    fn remove_deletes_row() {
        let (_dir, path) = tmp();
        let p = Persistence::open(&path).unwrap();
        let d = sample(DownloadState::Completed);
        p.save(&d).unwrap();
        assert!(p.list().unwrap().iter().any(|x| x.id == d.id));
        p.remove(d.id).unwrap();
        assert!(!p.list().unwrap().iter().any(|x| x.id == d.id));
    }

    #[test]
    fn list_returns_saved_rows() {
        let (_dir, path) = tmp();
        let p = Persistence::open(&path).unwrap();
        let a = sample(DownloadState::Queued);
        let b = sample(DownloadState::Queued);
        p.save(&a).unwrap();
        p.save(&b).unwrap();
        assert_eq!(p.list().unwrap().len(), 2);
    }
}
