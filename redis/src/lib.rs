//! Deadpool simple async pool for Redis connections.
//!
//! This crate implements a [`deadpool`](https://crates.io/crates/deadpool)
//! manager for [`redis`](https://crates.io/crates/redis).
//!
//! You should not need to use `deadpool` directly. Use the `Pool` type
//! provided by this crate instead.
//!
//! # Example
//!
//! ```rust
//! use std::env;
//!
//! use deadpool_redis::{cmd, Config};
//!
//! #[tokio::main]
//! async fn main() {
//!     let cfg = Config::from_env("REDIS").unwrap();
//!     let pool = cfg.create_pool().unwrap();
//!     {
//!         let mut conn = pool.get().await.unwrap();
//!         cmd("SET")
//!             .arg(&["deadpool/test_key", "42"])
//!             .execute_async(&mut conn)
//!             .await.unwrap();
//!     }
//!     {
//!         let mut conn = pool.get().await.unwrap();
//!         let value: String = cmd("GET")
//!             .arg(&["deadpool/test_key"])
//!             .query_async(&mut conn)
//!             .await.unwrap();
//!         assert_eq!(value, "42".to_string());
//!     }
//! }
//! ```
#![warn(missing_docs)]

use std::ops::{Deref, DerefMut};

use async_trait::async_trait;
use redis::{
    aio::Connection as RedisConnection, Client, IntoConnectionInfo, RedisError, RedisResult,
};

/// A type alias for using `deadpool::Pool` with `redis`
pub type Pool = deadpool::managed::Pool<ConnectionWrapper, RedisError>;

/// A type alias for using `deadpool::PoolError` with `redis`
pub type PoolError = deadpool::managed::PoolError<RedisError>;

type RecycleResult = deadpool::managed::RecycleResult<RedisError>;

mod config;
pub use config::Config;
mod cmd_wrapper;
pub use cmd_wrapper::{cmd, Cmd};
mod pipeline_wrapper;
pub use pipeline_wrapper::{pipe, Pipeline};

/// A type alias for using `deadpool::Object` with `redis`

/// A wrapper for `redis::Connection`. The `query_async` and `execute_async`
/// functions of `redis::Cmd` and `redis::Pipeline` consume the connection.
/// This wrapper makes it possible to replace the internal connection after
/// executing a query.
pub struct ConnectionWrapper {
    conn: RedisConnection,
}

impl Deref for ConnectionWrapper {
    type Target = RedisConnection;
    fn deref(&self) -> &RedisConnection {
        &self.conn
    }
}

impl DerefMut for ConnectionWrapper {
    fn deref_mut(&mut self) -> &mut RedisConnection {
        &mut self.conn
    }
}

/// The manager for creating and recyling lapin connections
pub struct Manager {
    client: Client,
}

impl Manager {
    /// Create manager using `PgConfig` and a `TlsConnector`
    pub fn new<T: IntoConnectionInfo>(params: T) -> RedisResult<Self> {
        Ok(Self {
            client: Client::open(params)?,
        })
    }
}

#[async_trait]
impl deadpool::managed::Manager<ConnectionWrapper, RedisError> for Manager {
    async fn create(&self) -> Result<ConnectionWrapper, RedisError> {
        let conn = self.client.get_async_connection().await?;
        Ok(ConnectionWrapper { conn })
    }

    async fn recycle(&self, conn: &mut ConnectionWrapper) -> RecycleResult {
        match cmd("PING").execute_async(conn).await {
            Ok(_) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}
