use async_trait::async_trait;
use fred::types::Key;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::convert::Infallible;
use std::future::Future;
use std::hint::unreachable_unchecked;

pub enum CacheBehaviour<T> {
    /// Cache this value.
    Cache(T),
    /// Do not cache this value.
    NoCache(T),
}

#[async_trait]
/// Trait for caching data in Redis, and calculating it if it's not in the cache yet.
pub trait ScCache: Send + Sync {
    type Error: Send + Sync;

    /// Do a cache lookup, on miss, calculate the value.
    async fn cached<S, Fn, Ft, T>(&self, cache_key: S, func: Fn) -> Result<T, Self::Error>
    where
        S: AsRef<str> + Into<Key> + Send + Sync,
        Fn: (FnOnce() -> Ft) + Send,
        Ft: Future<Output = CacheBehaviour<T>> + Send,
        T: DeserializeOwned + Serialize + Send + Sync,
    {
        match self
            .cached_may_fail(cache_key, || async {
                let r: Result<CacheBehaviour<T>, Infallible> = Ok(func().await);
                r
            })
            .await
        {
            Ok(Ok(v)) => Ok(v),
            // SAFETY: Since the closure above will never return an Err, we can mark this as
            // definitely unreachable.
            Ok(Err(_)) => unsafe { unreachable_unchecked() },
            Err(e) => Err(e),
        }
    }

    /// Do a cache lookup, on miss, calculate the value. Calculating the value may fail,
    /// in that case chain the error (= it has the same type as Self::Error).
    async fn cached_may_fail_chain<S, Fn, Ft, T>(
        &self,
        cache_key: S,
        func: Fn,
    ) -> Result<T, Self::Error>
    where
        S: AsRef<str> + Into<Key> + Send + Sync,
        Fn: (FnOnce() -> Ft) + Send,
        Ft: Future<Output = Result<CacheBehaviour<T>, Self::Error>> + Send,
        T: DeserializeOwned + Serialize + Send + Sync,
    {
        match self.cached_may_fail(cache_key, func).await {
            Ok(Ok(v)) => Ok(v),
            Ok(Err(e)) => Err(e),
            Err(e) => Err(e),
        }
    }

    /// Do a cache lookup, on miss, calculate the value. Calculating the value may fail.
    async fn cached_may_fail<S, Fn, Ft, T, E>(
        &self,
        cache_key: S,
        func: Fn,
    ) -> Result<Result<T, E>, Self::Error>
    where
        S: AsRef<str> + Into<Key> + Send + Sync,
        Fn: (FnOnce() -> Ft) + Send,
        Ft: Future<Output = Result<CacheBehaviour<T>, E>> + Send,
        T: DeserializeOwned + Serialize + Send + Sync,
        E: Send;
}

#[async_trait]
impl<B: ScCache> ScCache for &B {
    type Error = B::Error;

    async fn cached_may_fail<S, Fn, Ft, T, E>(
        &self,
        cache_key: S,
        func: Fn,
    ) -> Result<Result<T, E>, Self::Error>
    where
        S: AsRef<str> + Into<Key> + Send + Sync,
        Fn: (FnOnce() -> Ft) + Send,
        Ft: Future<Output = Result<CacheBehaviour<T>, E>> + Send,
        T: DeserializeOwned + Serialize + Send + Sync,
        E: Send,
    {
        <B as ScCache>::cached_may_fail(self, cache_key, func).await
    }
}
