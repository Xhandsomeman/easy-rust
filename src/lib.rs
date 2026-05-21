#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! easy-rust 提供低摩擦、生产级的 Rust API。

pub mod cache;
pub mod cmd;
pub mod codec;
pub mod compress;
pub mod config;
pub mod crawler;
pub mod crypto;
pub mod csv;
pub mod data;
pub mod fs;
pub mod json;
pub mod log;
pub mod prelude;
pub mod random;
pub mod regex;
pub mod request;
pub mod sqlite;
pub mod system;
pub mod text;
pub mod time;
pub mod url;
pub mod uuid;

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send_sync<T: Send + Sync>() {}

    #[test]
    fn public_errors_are_send_sync() {
        assert_send_sync::<cache::Error>();
        assert_send_sync::<cmd::Error>();
        assert_send_sync::<codec::Error>();
        assert_send_sync::<compress::Error>();
        assert_send_sync::<config::Error>();
        assert_send_sync::<crawler::Error>();
        assert_send_sync::<crypto::Error>();
        assert_send_sync::<csv::Error>();
        assert_send_sync::<data::Error>();
        assert_send_sync::<fs::Error>();
        assert_send_sync::<json::Error>();
        assert_send_sync::<log::Error>();
        assert_send_sync::<random::Error>();
        assert_send_sync::<regex::Error>();
        assert_send_sync::<request::Error>();
        assert_send_sync::<sqlite::Error>();
        assert_send_sync::<system::Error>();
        assert_send_sync::<text::Error>();
        assert_send_sync::<time::Error>();
        assert_send_sync::<url::Error>();
        assert_send_sync::<uuid::Error>();
    }
}
