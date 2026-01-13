//! DB primitives and connectors for Molt.

mod pool;

pub use pool::{AcquireError, Pool, Pooled};

#[cfg(feature = "async")]
mod async_pool;

#[cfg(feature = "async")]
pub use async_pool::{AsyncAcquireError, AsyncPool, AsyncPooled, CancelToken};

#[cfg(feature = "sqlite")]
mod sqlite;

#[cfg(feature = "sqlite")]
pub use sqlite::{sqlite_pool, SqliteConn, SqliteOpenMode};

#[cfg(feature = "postgres")]
mod postgres;

#[cfg(feature = "postgres")]
pub use postgres::{PgConn, PgPool, PgPoolConfig};
