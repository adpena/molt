//! DB primitives and connectors for Molt.

#[cfg(all(target_arch = "wasm32", feature = "sqlite"))]
compile_error!("molt-db sqlite support is not available on wasm yet (see ROADMAP.md)");

#[cfg(all(target_arch = "wasm32", feature = "postgres"))]
compile_error!("molt-db postgres support is not available on wasm yet (see ROADMAP.md)");

mod pool;

pub use pool::{AcquireError, Pool, Pooled};

#[cfg(feature = "async")]
mod async_pool;

#[cfg(feature = "async")]
pub use async_pool::{AsyncAcquireError, AsyncPool, AsyncPooled, CancelToken};

#[cfg(feature = "sqlite")]
mod sqlite;

#[cfg(feature = "sqlite")]
pub use sqlite::{SqliteConn, SqliteOpenMode, sqlite_pool};

/// Re-export the underlying `rusqlite` crate so downstream users (notably
/// `molt-runtime::builtins::sqlite3`) can build prepared statements, bind
/// parameters, and consume `ValueRef`s without pinning a separate version.
#[cfg(feature = "sqlite")]
pub use rusqlite;

#[cfg(feature = "postgres")]
mod postgres;

#[cfg(feature = "postgres")]
pub use postgres::{PgConn, PgPool, PgPoolConfig};
