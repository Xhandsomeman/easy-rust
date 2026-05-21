#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! easy-rust 提供低摩擦、生产级的 Rust API。

#[cfg(feature = "cache")]
pub mod cache;
#[cfg(feature = "cmd")]
pub mod cmd;
#[cfg(feature = "codec")]
pub mod codec;
#[cfg(feature = "compress")]
pub mod compress;
#[cfg(feature = "config")]
pub mod config;
#[cfg(feature = "crawler")]
pub mod crawler;
#[cfg(feature = "crypto")]
pub mod crypto;
#[cfg(feature = "csv")]
pub mod csv;
#[cfg(feature = "data")]
pub mod data;
#[cfg(feature = "fs")]
pub mod fs;
#[cfg(feature = "json")]
pub mod json;
#[cfg(feature = "log")]
pub mod log;
pub mod prelude;
#[cfg(feature = "random")]
pub mod random;
#[cfg(feature = "regex")]
pub mod regex;
#[cfg(feature = "request")]
pub mod request;
#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(feature = "system")]
pub mod system;
#[cfg(feature = "text")]
pub mod text;
#[cfg(any(feature = "time", feature = "time-lite"))]
pub mod time;
#[cfg(feature = "url")]
pub mod url;
#[cfg(feature = "uuid")]
pub mod uuid;

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn public_errors_are_send_sync() {
        #[cfg(feature = "cache")]
        assert_send_sync::<cache::Error>();
        #[cfg(feature = "cmd")]
        assert_send_sync::<cmd::Error>();
        #[cfg(feature = "codec")]
        assert_send_sync::<codec::Error>();
        #[cfg(feature = "compress")]
        assert_send_sync::<compress::Error>();
        #[cfg(feature = "config")]
        assert_send_sync::<config::Error>();
        #[cfg(feature = "crawler")]
        assert_send_sync::<crawler::Error>();
        #[cfg(feature = "crypto")]
        assert_send_sync::<crypto::Error>();
        #[cfg(feature = "csv")]
        assert_send_sync::<csv::Error>();
        #[cfg(feature = "data")]
        assert_send_sync::<data::Error>();
        #[cfg(feature = "fs")]
        assert_send_sync::<fs::Error>();
        #[cfg(feature = "json")]
        assert_send_sync::<json::Error>();
        #[cfg(feature = "log")]
        assert_send_sync::<log::Error>();
        #[cfg(feature = "random")]
        assert_send_sync::<random::Error>();
        #[cfg(feature = "regex")]
        assert_send_sync::<regex::Error>();
        #[cfg(feature = "request")]
        assert_send_sync::<request::Error>();
        #[cfg(feature = "sqlite")]
        assert_send_sync::<sqlite::Error>();
        #[cfg(feature = "system")]
        assert_send_sync::<system::Error>();
        #[cfg(feature = "text")]
        assert_send_sync::<text::Error>();
        #[cfg(any(feature = "time", feature = "time-lite"))]
        assert_send_sync::<time::Error>();
        #[cfg(feature = "url")]
        assert_send_sync::<url::Error>();
        #[cfg(feature = "uuid")]
        assert_send_sync::<uuid::Error>();
    }
}
