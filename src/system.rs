//! 极简系统信息 API。
//!
//! 这个模块用于读取当前运行环境的常见信息，例如操作系统、CPU 数、主机名和常用系统路径。
//! 路径统一返回 [`crate::fs::Path`]，不暴露底层路径类型。

use std::{env, error::Error as StdError, fmt, path::PathBuf};

use crate::fs::Path as FsPath;

/// system 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。常见写法是 `let name = system::hostname()?;`。
pub type Result<T> = std::result::Result<T, Error>;

/// system 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要定位系统信息读取失败的位置时，使用
/// [`Error::kind`]。
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl Error {
    fn new(kind: ErrorKind) -> Self {
        Self { kind, source: None }
    }

    fn with_source(kind: ErrorKind, source: impl StdError + Send + Sync + 'static) -> Self {
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
        self.source
            .as_deref()
            .map(|source| source as &(dyn StdError + 'static))
    }
}

/// system 模块的具体错误原因。
///
/// 错误信息会包含操作名和简短上下文，方便定位是哪个系统信息读取失败。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 系统信息读取失败。
    #[error("system {operation} failed: {message}")]
    Info {
        /// 发生错误的操作名，例如 `hostname`。
        operation: &'static str,
        /// 面向人的错误说明。
        message: String,
    },
}

/// 返回当前操作系统名称。
///
/// 返回值来自 Rust 标准库，例如 `macos`、`linux` 或 `windows`。
#[must_use]
pub fn os() -> &'static str {
    env::consts::OS
}

/// 返回当前 CPU 架构名称。
///
/// 返回值来自 Rust 标准库，例如 `aarch64`、`x86_64` 或 `arm`。
#[must_use]
pub fn arch() -> &'static str {
    env::consts::ARCH
}

/// 返回当前操作系统家族。
///
/// 常见返回值是 `unix` 或 `windows`，适合做简单平台分支。
#[must_use]
pub fn family() -> &'static str {
    env::consts::FAMILY
}

/// 返回当前可用 CPU 数。
///
/// 如果系统查询失败，会返回 `1`，避免调用方为这个很少见且通常不可恢复的情况写错误处理。
#[must_use]
pub fn cpu_count() -> usize {
    std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get)
}

/// 返回当前主机名。
///
/// 读取失败或主机名不是有效 UTF-8 时返回 [`ErrorKind::Info`]。
pub fn hostname() -> Result<String> {
    let name = hostname::get().map_err(|source| {
        Error::with_source(
            ErrorKind::Info {
                operation: "hostname",
                message: "read hostname failed".to_owned(),
            },
            source,
        )
    })?;
    name.into_string().map_err(|name| {
        ErrorKind::Info {
            operation: "hostname",
            message: format!("hostname is not valid UTF-8: {}", name.to_string_lossy()),
        }
        .into()
    })
}

/// 返回当前工作目录。
///
/// 成功时返回 easy-rust 的高层 [`crate::fs::Path`]，方便继续做文件操作。
pub fn current_dir() -> Result<FsPath> {
    let path = env::current_dir().map_err(|source| {
        Error::with_source(
            ErrorKind::Info {
                operation: "current_dir",
                message: "read current directory failed".to_owned(),
            },
            source,
        )
    })?;
    Ok(FsPath::from_std_path(&path))
}

/// 返回当前用户的 home 目录。
///
/// Unix 读取 `HOME`，Windows 读取 `USERPROFILE` 或 `HOMEDRIVE` + `HOMEPATH`。找不到时返回
/// [`ErrorKind::Info`]。
pub fn home_dir() -> Result<FsPath> {
    home_dir_path()
        .filter(|path| !path.as_os_str().is_empty())
        .map(|path| FsPath::from_std_path(&path))
        .ok_or_else(|| {
            ErrorKind::Info {
                operation: "home_dir",
                message: "home directory environment is not set".to_owned(),
            }
            .into()
        })
}

/// 返回当前可执行文件路径。
///
/// 成功时返回 easy-rust 的高层 [`crate::fs::Path`]。
pub fn exe() -> Result<FsPath> {
    let path = env::current_exe().map_err(|source| {
        Error::with_source(
            ErrorKind::Info {
                operation: "exe",
                message: "read executable path failed".to_owned(),
            },
            source,
        )
    })?;
    Ok(FsPath::from_std_path(&path))
}

#[cfg(windows)]
fn home_dir_path() -> Option<PathBuf> {
    env::var_os("USERPROFILE").map(PathBuf::from).or_else(|| {
        let drive = env::var_os("HOMEDRIVE")?;
        let path = env::var_os("HOMEPATH")?;
        let mut output = PathBuf::from(drive);
        output.push(path);
        Some(output)
    })
}

#[cfg(not(windows))]
fn home_dir_path() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    #[test]
    fn constants_are_non_empty() {
        assert!(!os().is_empty());
        assert!(!arch().is_empty());
        assert!(!family().is_empty());
        assert!(cpu_count() >= 1);
    }

    #[test]
    fn common_paths_are_high_level_paths() -> std::result::Result<(), Box<dyn StdError>> {
        assert!(current_dir()?.exists());
        assert!(exe()?.exists());
        if let Ok(home) = home_dir() {
            assert!(home.exists());
        }
        Ok(())
    }

    #[test]
    fn hostname_is_readable_or_has_operation_context() -> std::result::Result<(), Box<dyn StdError>>
    {
        match hostname() {
            Ok(name) => assert!(!name.is_empty()),
            Err(error) => match error.kind() {
                ErrorKind::Info { operation, .. } => assert_eq!(*operation, "hostname"),
            },
        }
        Ok(())
    }
}
