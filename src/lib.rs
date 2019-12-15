//! Deadpool is a dead simple async pool for connections and objects
//! of any type.
//!
//! # Example
//!
//! ```rust
//! use async_trait::async_trait;
//!
//! #[derive(Debug)]
//! enum Error { Fail }
//!
//! struct Connection {}
//!
//! type Pool = deadpool::Pool<Connection, Error>;
//!
//! impl Connection {
//!     async fn new() -> Result<Self, Error> {
//!         Ok(Connection {})
//!     }
//!     async fn check_health(&self) -> bool {
//!         true
//!     }
//!     async fn do_something(&self) -> String {
//!         "Hooray!".to_string()
//!     }
//! }
//!
//! struct Manager {}
//!
//! #[async_trait]
//! impl deadpool::Manager<Connection, Error> for Manager
//! {
//!     async fn create(&self) -> Result<Connection, Error> {
//!         Connection::new().await
//!     }
//!     async fn recycle(&self, conn: &mut Connection) -> deadpool::RecycleResult<Error> {
//!         if conn.check_health().await {
//!             Ok(())
//!         } else {
//!             Err(Error::Fail.into())
//!         }
//!     }
//! }
//!
//! #[tokio::main]
//! async fn main() {
//!     let mgr = Manager {};
//!     let pool = Pool::new(mgr, 16);
//!     let mut conn = pool.get().await.unwrap();
//!     let value = conn.do_something().await;
//!     assert_eq!(value, "Hooray!".to_string());
//! }
//! ```
//!
//! For a more complete example please see
//! [`deadpool-postgres`](https://crates.io/crates/deadpool-postgres)
#![warn(missing_docs)]

use std::future::Future;
use std::ops::{Deref, DerefMut};
use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};
use std::sync::{Arc, Weak};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::mpsc::{channel, Receiver, Sender};
use tokio::sync::Mutex;
use tokio::time::timeout;

mod config;
pub use config::PoolConfig;
mod errors;
pub use errors::{PoolError, RecycleError, TimeoutType};

/// Result type for the recycle function
pub type RecycleResult<E> = Result<(), RecycleError<E>>;

/// This trait is used to `create` new objects or `recycle` existing ones.
#[async_trait]
pub trait Manager<T, E> {
    /// Create a new instance of `T`
    async fn create(&self) -> Result<T, E>;
    /// Try to recycle an instance of `T` returning None` if the
    /// object could not be recycled.
    async fn recycle(&self, obj: &mut T) -> RecycleResult<E>;
}

enum ObjectState {
    New,
    Creating,
    Recycling,
    Ready,
}

/// A wrapper around the actual pooled object which implements the traits
/// `Deref`, `DerefMut` and `Drop`. Use this object just as if it was of type
/// `T` and upon leaving scope the `drop` function will take care of
/// returning it to the pool.
pub struct Object<T, E> {
    obj: Option<T>,
    state: ObjectState,
    pool: Weak<PoolInner<T, E>>,
}

impl<T, E> Object<T, E> {
    fn new(pool: &Pool<T, E>) -> Object<T, E> {
        Object {
            obj: None,
            state: ObjectState::New,
            pool: Arc::downgrade(&pool.inner),
        }
    }
}

impl<T, E> Drop for Object<T, E> {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.upgrade() {
            match self.state {
                ObjectState::New => {
                    pool.available.fetch_add(1, Ordering::Relaxed);
                }
                ObjectState::Creating => {
                    pool.available.fetch_add(1, Ordering::Relaxed);
                    pool.size.fetch_sub(1, Ordering::Relaxed);
                }
                ObjectState::Recycling => {
                    pool.available.fetch_add(1, Ordering::Relaxed);
                    if let Err(e) = pool.obj_sender.clone().try_send(self.obj.take()) {
                        pool.available.fetch_sub(1, Ordering::Relaxed);
                        pool.size.fetch_sub(1, Ordering::Relaxed);
                        // This code should be unreachable. Still if this ever
                        // happens fixing the pool state is a good idea.
                        if !std::thread::panicking() {
                            unreachable!("Could not return object to pool: {}", e);
                        }
                    }
                }
                ObjectState::Ready => {
                    pool.return_obj(self.obj.take());
                }
            }
        }
        self.obj = None;
        self.state = ObjectState::New;
    }
}

impl<T, E> Deref for Object<T, E> {
    type Target = T;
    fn deref(&self) -> &T {
        self.obj.as_ref().unwrap()
    }
}

impl<T, E> DerefMut for Object<T, E> {
    fn deref_mut(&mut self) -> &mut T {
        self.obj.as_mut().unwrap()
    }
}

struct PoolInner<T, E> {
    manager: Box<dyn Manager<T, E> + Sync + Send>,
    config: PoolConfig,
    obj_sender: Sender<Option<T>>,
    obj_receiver: Mutex<Receiver<Option<T>>>,
    size: AtomicUsize,
    /// The number of available objects in the pool. If there are no
    /// objects in the pool this number can become negative and stores the
    /// number of futures waiting for an object.
    available: AtomicIsize,
}

impl<T, E> PoolInner<T, E> {
    fn return_obj(&self, obj: Option<T>) {
        match self.obj_sender.clone().try_send(obj) {
            Ok(_) => {
                self.available.fetch_add(1, Ordering::Relaxed);
            }
            Err(e) => {
                // This code should be unreachable. Still if this ever
                // happens fixing the pool state is a good idea.
                self.size.fetch_sub(1, Ordering::Relaxed);
                if !std::thread::panicking() {
                    unreachable!("Could not return object to pool: {}", e);
                }
            }
        }
    }
}

/// A generic object and connection pool.
///
/// This struct can be cloned and transferred across thread boundaries
/// and uses reference counting for its internal state.
pub struct Pool<T, E> {
    inner: Arc<PoolInner<T, E>>,
}

#[derive(Debug)]
/// The current pool status.
pub struct Status {
    /// The size of the pool
    pub size: usize,
    /// The number of available objects in the pool. If there are no
    /// objects in the pool this number can become negative and stores the
    /// number of futures waiting for an object.
    pub available: isize,
}

impl<T, E> Clone for Pool<T, E> {
    fn clone(&self) -> Pool<T, E> {
        Pool {
            inner: self.inner.clone(),
        }
    }
}

impl<T, E> Pool<T, E> {
    /// Create new connection pool with a given `manager` and `max_size`.
    /// The `manager` is used to create and recycle objects and `max_size`
    /// is the maximum number of objects ever created.
    pub fn new(manager: impl Manager<T, E> + Send + Sync + 'static, max_size: usize) -> Pool<T, E> {
        Self::from_config(manager, PoolConfig::new(max_size))
    }
    /// Create new connection pool with a given `manager` and `config`.
    /// The `manager` is used to create and recycle objects and `max_size`
    /// is the maximum number of objects ever created.
    pub fn from_config(manager: impl Manager<T, E> + Send + Sync + 'static, config: PoolConfig) -> Pool<T, E> {
        let (obj_sender, obj_receiver) = channel::<Option<T>>(config.max_size);
        Pool {
            inner: Arc::new(PoolInner {
                manager: Box::new(manager),
                config: config,
                obj_sender: obj_sender,
                obj_receiver: Mutex::new(obj_receiver),
                size: AtomicUsize::new(0),
                available: AtomicIsize::new(0),
            }),
        }
    }
    /// Retrieve object from pool or wait for one to become available.
    pub async fn get(&self) -> Result<Object<T, E>, PoolError<E>> {
        let mut available = self.inner.available.fetch_sub(1, Ordering::Relaxed);
        let mut size = self.inner.size.load(Ordering::Relaxed);
        let mut obj = Object::new(&self);
        loop {
            if available <= 0 && size < self.inner.config.max_size {
                // The pool is empty and the max size has not been
                // reached, yet.
                if self.inner.size.fetch_add(1, Ordering::Relaxed) < self.inner.config.max_size {
                    self.inner.available.fetch_add(1, Ordering::Relaxed);
                    obj.state = ObjectState::Creating;
                    let create_future = self.inner.manager.create();
                    obj.obj = Some(apply_timeout(create_future, TimeoutType::Create, self.inner.config.create_timeout).await??);
                    obj.state = ObjectState::Ready;
                    break;
                } else {
                    self.inner.size.fetch_sub(1, Ordering::Relaxed);
                }
            }
            let inner_obj = apply_timeout(self._wait(), TimeoutType::Wait, self.inner.config.wait_timeout).await?;
            if let Some(inner_obj) = inner_obj {
                obj.obj = Some(inner_obj);
                obj.state = ObjectState::Recycling;
                let recycle_future = self.inner.manager.recycle(&mut obj);
                let recycle_result = apply_timeout(recycle_future, TimeoutType::Recycle, self.inner.config.recycle_timeout).await?;
                if recycle_result.is_ok() {
                    obj.state = ObjectState::Ready;
                    break;
                }
                obj.state = ObjectState::New;
            }
            // At this point either no object was received from the channel
            // or recycling the object failed. This means that the object
            // received from the channel was unuseable and the pool size
            // needs to be reduced by one.
            size = self.inner.size.fetch_sub(1, Ordering::Relaxed) - 1;
            available = self.inner.available.fetch_sub(1, Ordering::Relaxed);
        }
        Ok(obj)
    }
    async fn _wait(&self) -> Option<T> {
        self.inner.obj_receiver.lock().await.recv().await.unwrap()
    }
    /// Retrieve status of the pool
    pub fn status(&self) -> Status {
        let size = self.inner.size.load(Ordering::Relaxed);
        let available = self.inner.available.load(Ordering::Relaxed);
        Status { size, available }
    }
}

async fn apply_timeout<F, O, E>(future: F, timeout_type: TimeoutType, duration: Option<Duration>) -> Result<O, PoolError<E>>
where F: Future<Output = O>
{
    match duration {
        Some(duration) => match timeout(duration, future).await {
            Ok(result) => Ok(result),
            Err(elapsed) => Err(PoolError::Timeout(timeout_type, elapsed)),
        }
        None => Ok(future.await)
    }
}
