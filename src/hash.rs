//! 极简哈希 API。
//!
//! 这个模块只提供常用 SHA-2 摘要能力：字符串或字节的 `sha256`、`sha512`，以及文件 hash。
//! 它不是加密 API，不提供加密、解密、签名或密码存储能力。

use std::{error::Error as StdError, fmt, fs as std_fs};

use sha2::{Digest, Sha256, Sha512};

use crate::fs::Path as FsPath;

/// hash 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。当前主要用于读取文件失败。
pub type Result<T> = std::result::Result<T, Error>;

/// hash 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要定位文件读取失败时，使用 [`Error::kind`]。
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

/// hash 模块的具体错误原因。
///
/// 错误信息会包含操作名和路径，方便定位哪个文件读取失败。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 读取文件失败。
    #[error("hash {operation} `{path}` failed")]
    Read {
        /// 发生错误的操作名，例如 `file_sha256`。
        operation: &'static str,
        /// 发生错误的路径。
        path: FsPath,
    },
}

/// 计算 SHA-256 十六进制摘要。
///
/// 输入可以是 `&str`、`String`、`&[u8]` 或 `Vec<u8>`。
#[must_use]
pub fn sha256(input: impl AsRef<[u8]>) -> String {
    hex(Sha256::digest(input.as_ref()))
}

/// 计算 SHA-512 十六进制摘要。
///
/// 输入可以是 `&str`、`String`、`&[u8]` 或 `Vec<u8>`。
#[must_use]
pub fn sha512(input: impl AsRef<[u8]>) -> String {
    hex(Sha512::digest(input.as_ref()))
}

/// 读取文件并计算 SHA-256 十六进制摘要。
pub fn file_sha256(path: impl Into<FsPath>) -> Result<String> {
    let path = path.into();
    let bytes = read_file("file_sha256", &path)?;
    Ok(sha256(bytes))
}

/// 读取文件并计算 SHA-512 十六进制摘要。
pub fn file_sha512(path: impl Into<FsPath>) -> Result<String> {
    let path = path.into();
    let bytes = read_file("file_sha512", &path)?;
    Ok(sha512(bytes))
}

fn read_file(operation: &'static str, path: &FsPath) -> Result<Vec<u8>> {
    std_fs::read(path.as_std_path()).map_err(|source| {
        Error::with_source(
            ErrorKind::Read {
                operation,
                path: path.clone(),
            },
            source,
        )
    })
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let bytes = bytes.as_ref();
    let mut output = String::with_capacity(bytes.len() * 2);

    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }

    output
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error as StdError,
        fs as test_fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[test]
    fn sha256_and_sha512_match_known_values() {
        assert_eq!(
            sha256("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha512("abc"),
            "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a\
             2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"
                .replace(' ', "")
        );
    }

    #[test]
    fn file_hash_reads_file() -> std::result::Result<(), Box<dyn StdError>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let path =
            std::env::temp_dir().join(format!("easy-rust-hash-{}-{nanos}.txt", std::process::id()));
        test_fs::write(&path, "abc")?;

        let path = path.display().to_string();
        assert_eq!(file_sha256(&path)?, sha256("abc"));
        assert_eq!(file_sha512(&path)?, sha512("abc"));
        Ok(())
    }

    #[test]
    fn missing_file_returns_read_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match file_sha256("missing-hash-file.txt") {
            Ok(value) => return Err(format!("expected read error, got {value}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Read { path, .. } => assert_eq!(path.display(), "missing-hash-file.txt"),
        }

        Ok(())
    }
}
