//! Synchronous facade over the asynchronous [`turso`] SQLite engine.
//!
//! Stores in `navi-core` are synchronous (config loading, CLI startup, TUI
//! event loops, N-API entry points) while `turso` only exposes an async API.
//! This module owns a dedicated current-thread Tokio runtime and blocks on it,
//! so the async engine never leaks into store code.
//!
//! [`Db`] owns a database handle (file-backed or in-memory) and hands out
//! [`DbConnection`]s. Errors are mapped to [`anyhow::Error`] at the boundary;
//! row-mapping closures use [`RowResult`], which is `turso`'s own error type so
//! that `row.get(..)?` works naturally inside them.
//!
//! # Example
//!
//! ```no_run
//! use navi_core::db::{Db, params};
//!
//! # fn main() -> anyhow::Result<()> {
//! let db = Db::open("/tmp/example.db")?;
//! let conn = db.connect()?; // WAL + busy timeout + foreign keys ON
//! conn.execute(
//!     "CREATE TABLE IF NOT EXISTS kv (k TEXT PRIMARY KEY, v TEXT NOT NULL)",
//!     (),
//! )?;
//! conn.execute(
//!     "INSERT OR REPLACE INTO kv (k, v) VALUES (?1, ?2)",
//!     params!["hello", "world"],
//! )?;
//! let value: Option<String> = conn.query_row_optional(
//!     "SELECT v FROM kv WHERE k = ?1",
//!     params!["hello"],
//!     |row| row.get(0),
//! )?;
//! assert_eq!(value.as_deref(), Some("world"));
//! # Ok(())
//! # }
//! ```

use anyhow::{Context, Result};
use std::fmt::Display;
use std::future::Future;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;

pub use turso::Error as DbError;
pub use turso::params;
pub use turso::params_from_iter;
use turso::{IntoParams, Statement};
pub use turso::{Row, Value};

/// Result type returned by the row-mapping closures of [`DbConnection`].
pub type RowResult<T> = std::result::Result<T, DbError>;

/// Runtime dedicated to database work.
///
/// A current-thread runtime is enough: `turso` drives its own I/O threads and
/// only needs an executor to poll its futures. The runtime is process-wide so
/// every synchronous call site shares it.
fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("failed to start the navi db runtime")
    })
}

/// Drives `future` to completion on the dedicated database runtime.
///
/// This is the single bridge between the synchronous stores and `turso`'s
/// async API. It is safe to call from code already running inside a Tokio
/// runtime (for example `navi-napi` drives engine construction with its own
/// `Runtime::block_on`): on a multi-threaded ambient runtime the call is made
/// inside [`tokio::task::block_in_place`]; on a current-thread ambient runtime
/// the future runs on a scoped helper thread instead.
pub fn block_on<F>(future: F) -> F::Output
where
    F: Future + Send,
    F::Output: Send,
{
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| runtime().block_on(future))
        }
        // A current-thread runtime cannot block in place without stalling the
        // tasks it drives, so hand the future to a helper thread. The scoped
        // thread inherits no runtime context, which is exactly what we need.
        Ok(_) => std::thread::scope(|scope| {
            scope
                .spawn(|| runtime().block_on(future))
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
        }),
        Err(_) => runtime().block_on(future),
    }
}

/// Connection options applied by [`Db::connect_with`].
#[derive(Debug, Clone)]
pub struct OpenOptions {
    /// Run `PRAGMA journal_mode = WAL` (required for multi-process access).
    ///
    /// Ignored for in-memory databases.
    pub wal: bool,
    /// Run `PRAGMA foreign_keys = ON`.
    pub foreign_keys: bool,
    /// `PRAGMA synchronous` value (`None` keeps the SQLite default).
    pub synchronous: Option<String>,
    /// SQLite busy timeout; `Duration::ZERO` leaves the engine default.
    pub busy_timeout: Duration,
}

impl OpenOptions {
    /// No pragmas at all: the caller configures the connection itself.
    ///
    /// Used by stores that share a `configure_connection` helper.
    pub fn none() -> Self {
        Self {
            wal: false,
            foreign_keys: false,
            synchronous: None,
            busy_timeout: Duration::ZERO,
        }
    }
}

impl Default for OpenOptions {
    fn default() -> Self {
        Self {
            wal: true,
            foreign_keys: true,
            synchronous: None,
            busy_timeout: Duration::from_secs(5),
        }
    }
}

/// A local `turso` database (file-backed or in-memory).
///
/// The handle is cheap to keep around and is required to open connections.
pub struct Db {
    inner: turso::Database,
    in_memory: bool,
}

impl Db {
    /// Opens (or creates) the database file at `path`.
    ///
    /// Multi-process WAL coordination is enabled (`.tshm` sidecar), so the TUI
    /// and the CLI can keep the same database open at once.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let path_str = path
            .to_str()
            .with_context(|| format!("database path is not valid UTF-8: {}", path.display()))?;
        let inner = block_on(
            turso::Builder::new_local(path_str)
                .experimental_multiprocess_wal(true)
                .build(),
        )
        .with_context(|| format!("failed to open database at {}", path.display()))?;
        Ok(Self {
            inner,
            in_memory: false,
        })
    }

    /// Opens a private in-memory database (used by tests).
    pub fn open_in_memory() -> Result<Self> {
        let inner = block_on(turso::Builder::new_local(":memory:").build())
            .context("failed to open in-memory database")?;
        Ok(Self {
            inner,
            in_memory: true,
        })
    }

    /// Opens a connection with [`OpenOptions::default`].
    pub fn connect(&self) -> Result<DbConnection> {
        self.connect_with(&OpenOptions::default())
    }

    /// Opens a connection and applies `opts` (WAL, busy timeout, pragmas).
    pub fn connect_with(&self, opts: &OpenOptions) -> Result<DbConnection> {
        let conn = self
            .inner
            .connect()
            .context("failed to open database connection")?;
        if !opts.busy_timeout.is_zero() {
            conn.busy_timeout(opts.busy_timeout)
                .context("failed to set busy timeout")?;
        }
        if opts.wal && !self.in_memory {
            block_on(conn.pragma_update("journal_mode", "WAL"))
                .context("failed to enable WAL journal mode")?;
        }
        if opts.foreign_keys {
            block_on(conn.pragma_update("foreign_keys", "ON"))
                .context("failed to enable foreign keys")?;
        }
        if let Some(mode) = opts.synchronous.as_deref() {
            block_on(conn.pragma_update("synchronous", mode))
                .with_context(|| format!("failed to set synchronous = {mode}"))?;
        }
        Ok(DbConnection {
            conn,
            _db: self.inner.clone(),
        })
    }
}

/// A synchronous connection to a [`Db`].
///
/// All methods block the calling thread on the dedicated database runtime.
/// The database handle is kept alongside the connection so connections stay
/// valid even if the [`Db`] that created them is dropped.
pub struct DbConnection {
    conn: turso::Connection,
    /// Keeps the owning `turso::Database` alive for the connection's lifetime.
    /// Declared after `conn` so it drops last.
    _db: turso::Database,
}

impl DbConnection {
    /// Executes one statement, returning the number of changed rows.
    pub fn execute(&self, sql: &str, params: impl IntoParams + Send) -> Result<u64> {
        block_on(self.conn.execute(sql, params)).map_err(Into::into)
    }

    /// Executes a batch of statements separated by `;`.
    pub fn execute_batch(&self, sql: &str) -> Result<()> {
        block_on(self.conn.execute_batch(sql)).map_err(Into::into)
    }

    /// Runs a query expected to return exactly one row and maps it.
    ///
    /// Returns an error when the query yields no rows.
    pub fn query_row<T>(
        &self,
        sql: &str,
        params: impl IntoParams + Send,
        map: impl FnOnce(&Row) -> RowResult<T>,
    ) -> Result<T> {
        let row = block_on(async {
            let mut stmt = self.conn.prepare(sql).await?;
            stmt.query_row(params).await
        })
        .map_err(anyhow::Error::from)?;
        map(&row).map_err(Into::into)
    }

    /// Like [`query_row`](Self::query_row), but `None` when no row matches.
    pub fn query_row_optional<T>(
        &self,
        sql: &str,
        params: impl IntoParams + Send,
        map: impl FnOnce(&Row) -> RowResult<T>,
    ) -> Result<Option<T>> {
        let row = block_on(async {
            let mut stmt = self.conn.prepare(sql).await?;
            match stmt.query_row(params).await {
                Ok(row) => Ok(Some(row)),
                Err(DbError::QueryReturnedNoRows) => Ok(None),
                Err(err) => Err(err),
            }
        })
        .map_err(anyhow::Error::from)?;
        match row {
            Some(row) => map(&row).map(Some).map_err(Into::into),
            None => Ok(None),
        }
    }

    /// Runs a query and maps every returned row.
    pub fn query_rows<T>(
        &self,
        sql: &str,
        params: impl IntoParams + Send,
        mut map: impl FnMut(&Row) -> RowResult<T>,
    ) -> Result<Vec<T>> {
        self.collect_rows(sql, params)?
            .iter()
            .map(|row| map(row).map_err(anyhow::Error::from))
            .collect()
    }

    /// Runs `PRAGMA <name> = <value>`.
    ///
    /// The value is interpolated verbatim, exactly like `turso` does it: pass
    /// `WAL` for keywords, or a quoted literal (`'value'`) for strings.
    pub fn pragma_update(&self, pragma: &str, value: impl Display + Send) -> Result<()> {
        block_on(self.conn.pragma_update(pragma, value))
            .map(|_| ())
            .map_err(Into::into)
    }

    /// Runs `PRAGMA <name>` and maps every result row.
    pub fn pragma_query<T>(
        &self,
        pragma: &str,
        map: impl FnMut(&Row) -> RowResult<T>,
    ) -> Result<Vec<T>> {
        self.query_rows(&format!("PRAGMA {pragma}"), (), map)
    }

    /// Prepares a statement for repeated execution.
    pub fn prepare(&self, sql: &str) -> Result<DbStatement> {
        let stmt = block_on(self.conn.prepare(sql))?;
        Ok(DbStatement { stmt })
    }

    /// Starts a deferred transaction on this connection.
    ///
    /// The transaction rolls back when dropped without [`DbTransaction::commit`].
    pub fn unchecked_transaction(&self) -> Result<DbTransaction<'_>> {
        let tx = block_on(self.conn.unchecked_transaction())?;
        Ok(DbTransaction { tx })
    }

    /// Returns the rowid of the last inserted row on this connection.
    pub fn last_insert_rowid(&self) -> i64 {
        self.conn.last_insert_rowid()
    }

    /// Sets the total busy timeout for this connection.
    pub fn busy_timeout(&self, timeout: Duration) -> Result<()> {
        self.conn.busy_timeout(timeout).map_err(Into::into)
    }

    /// Returns `true` when the connection is not inside an explicit transaction.
    pub fn is_autocommit(&self) -> Result<bool> {
        self.conn.is_autocommit().map_err(Into::into)
    }

    /// Collects every result row of a query.
    fn collect_rows(&self, sql: &str, params: impl IntoParams + Send) -> Result<Vec<Row>> {
        block_on(async {
            let mut rows = self.conn.query(sql, params).await?;
            let mut out = Vec::new();
            while let Some(row) = rows.next().await? {
                out.push(row);
            }
            Ok::<Vec<Row>, DbError>(out)
        })
        .map_err(Into::into)
    }
}

/// A prepared statement created by [`DbConnection::prepare`] or
/// [`DbTransaction::prepare`].
pub struct DbStatement {
    stmt: Statement,
}

impl DbStatement {
    /// Executes the prepared statement with `params`.
    pub fn execute(&mut self, params: impl IntoParams + Send) -> Result<u64> {
        block_on(self.stmt.execute(params)).map_err(Into::into)
    }

    /// Number of rows changed by the most recent execution.
    pub fn n_change(&self) -> u64 {
        self.stmt.n_change()
    }

    /// Number of result columns of the prepared statement.
    pub fn column_count(&self) -> usize {
        self.stmt.column_count()
    }
}

/// A synchronous transaction created by [`DbConnection::unchecked_transaction`].
///
/// Statements prepared through [`prepare`](Self::prepare) are bound to the same
/// connection and therefore take part in the transaction.
pub struct DbTransaction<'conn> {
    tx: turso::transaction::Transaction<'conn>,
}

impl DbTransaction<'_> {
    /// Executes one statement inside the transaction.
    pub fn execute(&self, sql: &str, params: impl IntoParams + Send) -> Result<u64> {
        block_on(self.tx.execute(sql, params)).map_err(Into::into)
    }

    /// Prepares a statement inside the transaction.
    pub fn prepare(&self, sql: &str) -> Result<DbStatement> {
        let stmt = block_on(self.tx.prepare(sql))?;
        Ok(DbStatement { stmt })
    }

    /// Commits the transaction.
    pub fn commit(self) -> Result<()> {
        let tx = self.tx;
        block_on(tx.commit()).map_err(Into::into)
    }

    /// Rolls the transaction back.
    pub fn rollback(self) -> Result<()> {
        let tx = self.tx;
        block_on(tx.rollback()).map_err(Into::into)
    }
}
