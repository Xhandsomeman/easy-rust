//! 极简内存缓存 API。
//!
//! 这个模块提供进程内、线程安全、支持可选 TTL 的轻量缓存。它适合脚本、小服务和重复计算；
//! 第一版不做 Redis、分布式缓存、容量限制或淘汰策略。

use std::{
    collections::HashMap,
    error::Error as StdError,
    fmt,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

/// cache 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。当前主要用于 [`Cache::get_or`] 的加载函数失败。
pub type Result<T> = std::result::Result<T, Error>;

/// cache 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分加载失败时，使用 [`Error::kind`]。
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    source: Option<Box<dyn StdError + 'static>>,
}

impl Error {
    fn new(kind: ErrorKind) -> Self {
        Self { kind, source: None }
    }

    fn with_source(kind: ErrorKind, source: impl StdError + 'static) -> Self {
        Self {
            kind,
            source: Some(Box::new(source)),
        }
    }

    /// 返回具体错误类型。
    ///
    /// 调用方可以通过匹配 [`ErrorKind`] 做精细错误处理。
    #[must_use]
    pub fn kind(&self) -> &ErrorKind {
        &self.kind
    }
}

impl From<ErrorKind> for Error {
    fn from(kind: ErrorKind) -> Self {
        Self::new(kind)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.kind.fmt(formatter)?;
        if let Some(source) = &self.source {
            write!(formatter, ": {source}")?;
        }
        Ok(())
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source.as_deref()
    }
}

/// cache 模块的具体错误原因。
///
/// 错误信息会包含操作名和 key，不会输出缓存值内容。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// `get_or` 的加载函数执行失败。
    #[error("cache get_or `{key}` failed")]
    Load {
        /// 发生错误的缓存 key。
        key: String,
    },
}

/// 进程内线程安全缓存。
///
/// `Cache<T>` 是类型安全缓存；一个缓存实例只保存一种值类型。克隆缓存会共享同一份内部数据，
/// 适合在小服务的多个组件之间传递。
#[derive(Debug)]
pub struct Cache<T> {
    entries: Arc<Mutex<HashMap<String, Entry<T>>>>,
}

impl<T> Clone for Cache<T> {
    fn clone(&self) -> Self {
        Self {
            entries: Arc::clone(&self.entries),
        }
    }
}

impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> Cache<T> {
    /// 创建空缓存。
    ///
    /// 新缓存没有容量限制，也不会自动清理；过期数据会在读取、删除或 `get_or` 时懒清理。
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 写入永不过期的缓存值。
    ///
    /// 如果 key 已存在，会覆盖旧值并清除旧 TTL。
    pub fn set(&self, key: impl AsRef<str>, value: T) {
        self.insert(key.as_ref(), value, None);
    }

    /// 写入带 TTL 的缓存值。
    ///
    /// `ttl` 到期后，这个 key 会在下次读取、删除或 `get_or` 时按不存在处理。
    pub fn set_ttl(&self, key: impl AsRef<str>, value: T, ttl: Duration) {
        let now = Instant::now();
        let expires_at = now.checked_add(ttl).unwrap_or(now);
        self.insert(key.as_ref(), value, Some(expires_at));
    }

    /// 清空缓存中的全部 key。
    pub fn clear(&self) {
        self.lock_entries().clear();
    }

    fn insert(&self, key: &str, value: T, expires_at: Option<Instant>) {
        self.lock_entries()
            .insert(key.to_owned(), Entry { value, expires_at });
    }
}

impl<T> Cache<T>
where
    T: Clone,
{
    /// 读取缓存值。
    ///
    /// key 不存在或已经过期时返回 `None`。返回值是 owned clone，避免把内部锁暴露给调用方。
    #[must_use]
    pub fn get(&self, key: impl AsRef<str>) -> Option<T> {
        let key = key.as_ref();
        let mut entries = self.lock_entries();

        if is_expired(&entries, key) {
            entries.remove(key);
            return None;
        }

        entries.get(key).map(|entry| entry.value.clone())
    }

    /// 删除缓存值并返回旧值。
    ///
    /// key 不存在或已经过期时返回 `None`；过期 key 会被懒清理。
    pub fn remove(&self, key: impl AsRef<str>) -> Option<T> {
        let key = key.as_ref();
        let mut entries = self.lock_entries();

        if is_expired(&entries, key) {
            entries.remove(key);
            return None;
        }

        entries.remove(key).map(|entry| entry.value)
    }

    /// 读取缓存；未命中时调用加载函数并写入缓存。
    ///
    /// 命中缓存时不会执行 `loader`。未命中时，`loader` 成功返回的值会作为永不过期缓存写入；
    /// `loader` 失败会返回 [`ErrorKind::Load`]，错误中包含 key 和源错误。
    ///
    /// ```ignore
    /// let user = cache.get_or("user:1", || {
    ///     Ok::<_, std::io::Error>(User { name: "Ada".to_owned() })
    /// })?;
    /// ```
    pub fn get_or<E>(
        &self,
        key: impl AsRef<str>,
        loader: impl FnOnce() -> std::result::Result<T, E>,
    ) -> Result<T>
    where
        E: StdError + Send + Sync + 'static,
    {
        let key = key.as_ref();

        if let Some(value) = self.get(key) {
            return Ok(value);
        }

        let value = loader().map_err(|source| {
            Error::with_source(
                ErrorKind::Load {
                    key: key.to_owned(),
                },
                source,
            )
        })?;

        self.set(key, value.clone());
        Ok(value)
    }
}

impl<T> Cache<T> {
    fn lock_entries(&self) -> MutexGuard<'_, HashMap<String, Entry<T>>> {
        match self.entries.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

#[derive(Debug)]
struct Entry<T> {
    value: T,
    expires_at: Option<Instant>,
}

impl<T> Entry<T> {
    fn is_expired(&self, now: Instant) -> bool {
        self.expires_at.is_some_and(|expires_at| now >= expires_at)
    }
}

fn is_expired<T>(entries: &HashMap<String, Entry<T>>, key: &str) -> bool {
    let now = Instant::now();
    entries.get(key).is_some_and(|entry| entry.is_expired(now))
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, error::Error as StdError, fmt};

    use super::*;

    #[derive(Debug)]
    struct LoadFailed;

    impl fmt::Display for LoadFailed {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(formatter, "load failed")
        }
    }

    impl StdError for LoadFailed {}

    #[test]
    fn set_get_remove_and_clear_work() {
        let cache = Cache::new();

        cache.set("user:1", "Ada".to_owned());
        assert_eq!(cache.get("user:1"), Some("Ada".to_owned()));

        assert_eq!(cache.remove("user:1"), Some("Ada".to_owned()));
        assert_eq!(cache.get("user:1"), None);

        cache.set("user:2", "Grace".to_owned());
        cache.clear();
        assert_eq!(cache.get("user:2"), None);
    }

    #[test]
    fn cloned_cache_shares_entries() {
        let cache = Cache::new();
        let cloned = cache.clone();

        cache.set("answer", 42);

        assert_eq!(cloned.get("answer"), Some(42));
    }

    #[test]
    fn ttl_values_expire_lazily() {
        let cache = Cache::new();

        cache.set_ttl("fresh", "value".to_owned(), Duration::from_secs(60));
        cache.set_ttl("expired", "gone".to_owned(), Duration::from_secs(0));

        assert_eq!(cache.get("fresh"), Some("value".to_owned()));
        assert_eq!(cache.get("expired"), None);
    }

    #[test]
    fn remove_expired_key_returns_none_and_cleans_it() {
        let cache = Cache::new();

        cache.set_ttl("expired", "gone".to_owned(), Duration::from_secs(0));

        assert_eq!(cache.remove("expired"), None);
        assert_eq!(cache.get("expired"), None);
    }

    #[test]
    fn get_or_does_not_call_loader_when_cache_hits() -> std::result::Result<(), Box<dyn StdError>> {
        let cache = Cache::new();
        let calls = Cell::new(0);

        cache.set("key", "cached".to_owned());
        let value = cache.get_or("key", || {
            calls.set(calls.get() + 1);
            Ok::<_, LoadFailed>("loaded".to_owned())
        })?;

        assert_eq!(value, "cached");
        assert_eq!(calls.get(), 0);
        Ok(())
    }

    #[test]
    fn get_or_fetches_and_caches_missing_value() -> std::result::Result<(), Box<dyn StdError>> {
        let cache = Cache::new();
        let calls = Cell::new(0);

        let value = cache.get_or("key", || {
            calls.set(calls.get() + 1);
            Ok::<_, LoadFailed>("loaded".to_owned())
        })?;

        assert_eq!(value, "loaded");
        assert_eq!(cache.get("key"), Some("loaded".to_owned()));
        assert_eq!(calls.get(), 1);
        Ok(())
    }

    #[test]
    fn get_or_wraps_loader_error() -> std::result::Result<(), Box<dyn StdError>> {
        let cache = Cache::<String>::new();

        let error = match cache.get_or("key", || Err::<String, _>(LoadFailed)) {
            Ok(value) => return Err(format!("expected load error, got {value}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Load { key } => {
                assert_eq!(key, "key");
            }
        }
        assert_eq!(
            StdError::source(&error).map(ToString::to_string).as_deref(),
            Some("load failed")
        );

        assert_eq!(cache.get("key"), None);
        Ok(())
    }
}
