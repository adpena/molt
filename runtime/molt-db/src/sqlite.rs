//! SQLite connector helpers for the Molt DB layer.

use crate::Pool;
use rusqlite::{Connection, OpenFlags};
use std::path::Path;
use std::time::Duration;

const BUSY_TIMEOUT_MS: u64 = 100;

#[derive(Clone, Copy, Debug)]
pub enum SqliteOpenMode {
    ReadOnly,
    ReadWrite,
}

pub struct SqliteConn {
    conn: Connection,
}

impl SqliteConn {
    pub fn open(path: &Path, mode: SqliteOpenMode) -> Result<Self, rusqlite::Error> {
        let flags = match mode {
            SqliteOpenMode::ReadOnly => OpenFlags::SQLITE_OPEN_READ_ONLY,
            SqliteOpenMode::ReadWrite => {
                OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE
            }
        };
        let conn = Connection::open_with_flags(path, flags)?;
        conn.busy_timeout(Duration::from_millis(BUSY_TIMEOUT_MS))?;
        if matches!(mode, SqliteOpenMode::ReadOnly) {
            conn.pragma_update(None, "query_only", 1)?;
        }
        Ok(Self { conn })
    }

    pub fn open_read_only(path: &Path) -> Result<Self, rusqlite::Error> {
        Self::open(path, SqliteOpenMode::ReadOnly)
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }
}

pub fn sqlite_pool(
    path: &Path,
    pool_size: usize,
    mode: SqliteOpenMode,
) -> std::sync::Arc<Pool<SqliteConn>> {
    let path = path.to_path_buf();
    Pool::new(pool_size, move || {
        SqliteConn::open(&path, mode).expect("sqlite open failed")
    })
}
