use std::{fmt, path::PathBuf};

use deadpool::{managed::BuildError, Runtime};
#[cfg(feature = "serde")]
use serde_1::Deserialize;

use crate::{Pool, PoolConfig, RedisResult};

/// Configuration object.
///
/// # Example (from environment)
///
/// By enabling the `serde` feature you can read the configuration using the
/// [`config`](https://crates.io/crates/config) crate as following
/// ```env
/// REDIS__CONNECTION__ADDR=redis.example.com
/// REDIS__POOL__MAX_SIZE=16
/// REDIS__POOL__TIMEOUTS__WAIT__SECS=2
/// REDIS__POOL__TIMEOUTS__WAIT__NANOS=0
/// ```
///
/// ```rust
/// struct Config {
///     redis: deadpool_redis::Config,
/// }
///
/// impl Config {
///     pub fn from_env() -> Result<Self, config::ConfigError> {
///         let mut cfg = config::Config::new();
///         cfg.merge(config::Environment::new().separator("__")).unwrap();
///         cfg.try_into().unwrap()
///     }
/// }
/// ```
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(serde_1::Deserialize))]
#[cfg_attr(feature = "serde", serde(crate = "serde_1"))]
pub struct Config {
    /// Redis URL.
    ///
    /// See [Connection Parameters](redis#connection-parameters).
    pub url: Option<String>,

    /// [`redis::ConnectionInfo`] structure.
    pub connection: Option<ConnectionInfo>,

    /// Pool configuration.
    pub pool: Option<PoolConfig>,
}

impl Config {
    /// Creates a new [`Pool`] using this [`Config`].
    ///
    /// # Errors
    ///
    /// See [`BuildError`] and [`RedisError`] for details.
    ///
    /// [`RedisError`]: redis::RedisError
    pub fn create_pool(&self, runtime: Runtime) -> Result<Pool, BuildError<redis::RedisError>> {
        let manager = match (&self.url, &self.connection) {
            (Some(url), None) => crate::Manager::new(url.as_str())?,
            (None, Some(connection)) => crate::Manager::new(connection.clone())?,
            (None, None) => crate::Manager::new(ConnectionInfo::default())?,
            (Some(_), Some(_)) => {
                return Err(BuildError::Config(
                    "url and connection must not be specified at the same time.".to_owned(),
                ))
            }
        };
        let pool_config = self.get_pool_config();
        Pool::builder(manager)
            .config(pool_config)
            .runtime(runtime)
            .build()
    }

    /// Returns [`deadpool::managed::PoolConfig`] which can be used to construct
    /// a [`deadpool::managed::Pool`] instance.
    #[must_use]
    pub fn get_pool_config(&self) -> PoolConfig {
        self.pool.clone().unwrap_or_default()
    }

    /// Creates a new [`Config`] from the given Redis URL (like
    /// `redis://127.0.0.1`).
    #[must_use]
    pub fn from_url<T: Into<String>>(url: T) -> Config {
        Config {
            url: Some(url.into()),
            connection: None,
            pool: None,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            url: None,
            connection: Some(ConnectionInfo::default()),
            pool: None,
        }
    }
}

/// This is a 1:1 copy of the [`redis::ConnectionAddr`] enumeration.
/// This is duplicated here in order to add support for the
/// [`serde::Deserialize`] trait which is required for the [`serde`] support.
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde", derive(Deserialize))]
#[cfg_attr(feature = "serde", serde(crate = "serde_1"))]
pub enum ConnectionAddr {
    /// Format for this is `(host, port)`.
    Tcp(String, u16),

    /// Format for this is `(host, port)`.
    TcpTls {
        /// Hostname.
        host: String,

        /// Port.
        port: u16,

        /// Disable hostname verification when connecting.
        ///
        /// # Warning
        ///
        /// You should think very carefully before you use this method. If
        /// hostname verification is not used, any valid certificate for any
        /// site will be trusted for use from any other. This introduces a
        /// significant vulnerability to man-in-the-middle attacks.
        insecure: bool,
    },

    /// Format for this is the path to the unix socket.
    Unix(PathBuf),
}

impl Default for ConnectionAddr {
    fn default() -> Self {
        Self::Tcp("127.0.0.1".to_string(), 6379)
    }
}

impl From<ConnectionAddr> for redis::ConnectionAddr {
    fn from(addr: ConnectionAddr) -> Self {
        match addr {
            ConnectionAddr::Tcp(host, port) => Self::Tcp(host, port),
            ConnectionAddr::TcpTls {
                host,
                port,
                insecure,
            } => Self::TcpTls {
                host,
                port,
                insecure,
            },
            ConnectionAddr::Unix(path) => Self::Unix(path),
        }
    }
}

impl From<redis::ConnectionAddr> for ConnectionAddr {
    fn from(addr: redis::ConnectionAddr) -> Self {
        match addr {
            redis::ConnectionAddr::Tcp(host, port) => Self::Tcp(host, port),
            redis::ConnectionAddr::TcpTls {
                host,
                port,
                insecure,
            } => ConnectionAddr::TcpTls {
                host,
                port,
                insecure,
            },
            redis::ConnectionAddr::Unix(path) => Self::Unix(path),
        }
    }
}

/// This is a 1:1 copy of the [`redis::ConnectionInfo`] struct.
/// This is duplicated here in order to add support for the
/// [`serde::Deserialize`] trait which is required for the [`serde`] support.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Deserialize))]
#[cfg_attr(feature = "serde", serde(crate = "serde_1"))]
pub struct ConnectionInfo {
    /// A connection address for where to connect to.
    pub addr: ConnectionAddr,

    /// A boxed connection address for where to connect to.
    #[cfg_attr(feature = "serde", serde(flatten))]
    pub redis: RedisConnectionInfo,
}

impl From<ConnectionInfo> for redis::ConnectionInfo {
    fn from(info: ConnectionInfo) -> Self {
        Self {
            addr: info.addr.into(),
            redis: info.redis.into(),
        }
    }
}

impl From<redis::ConnectionInfo> for ConnectionInfo {
    fn from(info: redis::ConnectionInfo) -> Self {
        Self {
            addr: info.addr.into(),
            redis: info.redis.into(),
        }
    }
}

impl redis::IntoConnectionInfo for ConnectionInfo {
    fn into_connection_info(self) -> RedisResult<redis::ConnectionInfo> {
        Ok(self.into())
    }
}

/// This is a 1:1 copy of the [`redis::RedisConnectionInfo`] struct.
/// This is duplicated here in order to add support for the
/// [`serde::Deserialize`] trait which is required for the [`serde`] support.
#[derive(Clone, Debug, Default)]
#[cfg_attr(feature = "serde", derive(Deserialize))]
#[cfg_attr(feature = "serde", serde(crate = "serde_1"))]
pub struct RedisConnectionInfo {
    /// The database number to use. This is usually `0`.
    pub db: i64,

    /// Optionally a username that should be used for connection.
    pub username: Option<String>,

    /// Optionally a password that should be used for connection.
    pub password: Option<String>,
}

impl From<RedisConnectionInfo> for redis::RedisConnectionInfo {
    fn from(info: RedisConnectionInfo) -> Self {
        Self {
            db: info.db,
            username: info.username,
            password: info.password,
        }
    }
}

impl From<redis::RedisConnectionInfo> for RedisConnectionInfo {
    fn from(info: redis::RedisConnectionInfo) -> Self {
        Self {
            db: info.db,
            username: info.username,
            password: info.password,
        }
    }
}

/// Possible errors returned when [`Pool`] creation fails.
#[derive(Debug)]
pub enum CreatePoolError {
    /// [`PoolConfig`] contained invalid options.
    Config(String),

    /// Redis returned an error while creating the [`Pool`].
    Redis(redis::RedisError),
}

impl From<redis::RedisError> for CreatePoolError {
    fn from(error: redis::RedisError) -> Self {
        Self::Redis(error)
    }
}

impl fmt::Display for CreatePoolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(msg) => write!(f, "Config error: {}", msg),
            Self::Redis(err) => write!(f, "Config error: {}", err),
        }
    }
}

impl std::error::Error for CreatePoolError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(_) => None,
            Self::Redis(err) => Some(err),
        }
    }
}
