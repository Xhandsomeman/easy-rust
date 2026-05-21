//! 极简文件系统 API。
//!
//! 这个模块把文件读写、JSON 文件、目录、临时路径和路径对象合并到一个入口里。用户只需要记住
//! `easy_rust::fs`，不需要在 `fs` 和 `path` 两套概念之间切换。

use std::{
    error::Error as StdError,
    fmt, fs as std_fs, io,
    path::{Path as StdPath, PathBuf},
};

use serde::{Serialize, de::DeserializeOwned};

const TEMP_ATTEMPTS: usize = 100;

/// fs 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。常见写法是 `fn main() -> fs::Result<()>`，
/// 然后用 `?` 把文件系统错误继续向上传递。
pub type Result<T> = std::result::Result<T, Error>;

/// fs 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分读写、JSON、目录等错误时，使用
/// [`Error::kind`]。
#[derive(Debug)]
pub struct Error {
    kind: Box<ErrorKind>,
    source: Option<Box<dyn StdError + 'static>>,
}

impl Error {
    fn new(kind: ErrorKind) -> Self {
        Self {
            kind: Box::new(kind),
            source: None,
        }
    }

    fn with_source(kind: ErrorKind, source: impl StdError + 'static) -> Self {
        Self {
            kind: Box::new(kind),
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

/// fs 模块的具体错误原因。
///
/// 每个错误都带有操作名和路径，方便定位是哪一步文件操作失败。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 读取文件失败。
    #[error("fs read `{path}` failed")]
    Read {
        /// 发生错误的路径。
        path: Path,
    },

    /// 写入文件失败。
    #[error("fs write `{path}` failed")]
    Write {
        /// 发生错误的路径。
        path: Path,
    },

    /// 创建目录失败。
    #[error("fs make_dir `{path}` failed")]
    MakeDir {
        /// 发生错误的路径。
        path: Path,
    },

    /// 删除路径失败。
    #[error("fs remove `{path}` failed")]
    Remove {
        /// 发生错误的路径。
        path: Path,
    },

    /// 列出目录失败。
    #[error("fs list_dir `{path}` failed")]
    ListDir {
        /// 发生错误的路径。
        path: Path,
    },

    /// JSON 序列化失败。
    #[error("fs json_encode `{path}` failed")]
    JsonEncode {
        /// 发生错误的路径。
        path: Path,
    },

    /// JSON 解析失败。
    #[error("fs json_decode `{path}` failed")]
    JsonDecode {
        /// 发生错误的路径。
        path: Path,
    },

    /// 临时路径创建失败。
    #[error("fs {operation} `{path}` failed: {message}")]
    Temp {
        /// 发生错误的操作名，例如 `temp_dir`。
        operation: &'static str,
        /// 发生错误的路径。
        path: Path,
        /// 面向人的错误说明。
        message: String,
    },

    /// 复制文件或目录失败。
    #[error("fs copy `{from}` to `{to}` failed")]
    Copy {
        /// 源路径。
        from: Path,
        /// 目标路径。
        to: Path,
    },

    /// 移动文件或目录失败。
    #[error("fs move_path `{from}` to `{to}` failed")]
    Move {
        /// 源路径。
        from: Path,
        /// 目标路径。
        to: Path,
    },

    /// 遍历目录失败。
    #[error("fs walk `{path}` failed")]
    Walk {
        /// 发生错误的路径。
        path: Path,
    },

    /// 读取路径信息失败。
    #[error("fs {operation} `{path}` failed")]
    Info {
        /// 发生错误的操作名，例如 `size` 或 `absolute`。
        operation: &'static str,
        /// 发生错误的路径。
        path: Path,
    },

    /// 文件锁操作失败。
    #[error("fs {operation} `{path}` failed")]
    Lock {
        /// 发生错误的操作名，例如 `lock` 或 `unlock`。
        operation: &'static str,
        /// 发生错误的路径。
        path: Path,
    },
}

/// easy-rust 的高层路径对象。
///
/// 它只服务文件操作。需要连续操作同一路径时，使用 `fs::path(...)` 得到这个对象。
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct Path {
    inner: PathBuf,
}

impl Path {
    pub(crate) fn as_std_path(&self) -> &StdPath {
        &self.inner
    }

    pub(crate) fn from_std_path(path: &StdPath) -> Self {
        Self {
            inner: path.to_path_buf(),
        }
    }

    /// 返回一个拼接子路径后的新路径。
    ///
    /// 适合从目录路径继续定位文件，例如 `fs::path("data").join("user.json")`。
    #[must_use]
    pub fn join(&self, name: impl AsRef<str>) -> Self {
        Self {
            inner: self.inner.join(name.as_ref()),
        }
    }

    /// 返回一个替换文件名后的新路径。
    ///
    /// 适合把 `data/user.json` 替换成同目录下的 `backup.json`。
    #[must_use]
    pub fn with_name(&self, name: impl AsRef<str>) -> Self {
        Self {
            inner: self.inner.with_file_name(name.as_ref()),
        }
    }

    /// 返回文件名。
    ///
    /// 如果路径没有文件名，或文件名不是有效 UTF-8，则返回 `None`。
    #[must_use]
    pub fn name(&self) -> Option<String> {
        self.inner
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
    }

    /// 返回适合显示给用户的路径字符串。
    ///
    /// 这个方法用于日志、错误信息和测试断言。
    #[must_use]
    pub fn display(&self) -> String {
        self.inner.display().to_string()
    }

    /// 读取路径对应文件的 UTF-8 文本。
    pub fn read_text(&self) -> Result<String> {
        read_text(self)
    }

    /// 读取路径对应文件的文本行。
    ///
    /// 返回值不包含换行符；空行会保留为空字符串。
    pub fn read_lines(&self) -> Result<Vec<String>> {
        read_lines(self)
    }

    /// 写入 UTF-8 文本，并自动创建父目录。
    pub fn write_text(&self, text: impl AsRef<str>) -> Result<()> {
        write_text(self, text)
    }

    /// 写入多行文本，并自动创建父目录。
    ///
    /// 每一行都会写入一个 `\n`，空列表会创建空文件。
    pub fn write_lines<S>(&self, lines: impl IntoIterator<Item = S>) -> Result<()>
    where
        S: AsRef<str>,
    {
        write_lines(self, lines)
    }

    /// 读取路径对应文件的字节内容。
    pub fn read_bytes(&self) -> Result<Vec<u8>> {
        read_bytes(self)
    }

    /// 写入字节内容，并自动创建父目录。
    pub fn write_bytes(&self, bytes: impl AsRef<[u8]>) -> Result<()> {
        write_bytes(self, bytes)
    }

    /// 读取路径对应文件并解析为 JSON 类型。
    pub fn read_json<T>(&self) -> Result<T>
    where
        T: DeserializeOwned,
    {
        read_json(self)
    }

    /// 把值写成 pretty JSON，并自动创建父目录。
    pub fn write_json<T>(&self, value: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        write_json(self, value)
    }

    /// 判断路径是否存在。
    ///
    /// 这个方法不返回错误；权限不足等无法确认的情况按不存在处理。
    #[must_use]
    pub fn exists(&self) -> bool {
        exists(self)
    }

    /// 创建目录及其所有父目录。
    pub fn make_dir(&self) -> Result<()> {
        make_dir(self)
    }

    /// 列出目录下的直接子路径。
    ///
    /// 返回结果按显示字符串排序，保证测试和脚本输出稳定。
    pub fn list_dir(&self) -> Result<Vec<Path>> {
        list_dir(self)
    }

    /// 删除文件或目录。
    ///
    /// 文件会直接删除，目录会递归删除；路径不存在时返回成功。
    pub fn remove(&self) -> Result<()> {
        remove(self)
    }

    /// 复制当前路径到目标路径。
    pub fn copy_to(&self, to: impl Into<Path>) -> Result<()> {
        copy(self, to)
    }

    /// 移动当前路径到目标路径。
    pub fn move_to(&self, to: impl Into<Path>) -> Result<()> {
        move_path(self, to)
    }

    /// 递归列出当前目录下的所有子路径。
    pub fn walk(&self) -> Result<Vec<Path>> {
        walk(self)
    }

    /// 返回文件大小，目录会返回其内部普通文件大小总和。
    pub fn size(&self) -> Result<u64> {
        size(self)
    }

    /// 返回父目录路径。
    ///
    /// 没有父目录时返回 `None`。
    #[must_use]
    pub fn parent(&self) -> Option<Path> {
        self.inner.parent().map(Self::from_std_path)
    }

    /// 返回扩展名。
    ///
    /// 扩展名不包含 `.`；没有扩展名或扩展名不是有效 UTF-8 时返回 `None`。
    #[must_use]
    pub fn ext(&self) -> Option<String> {
        self.inner
            .extension()
            .and_then(|value| value.to_str())
            .map(ToOwned::to_owned)
    }

    /// 返回不带扩展名的文件名。
    ///
    /// 没有文件名或文件名不是有效 UTF-8 时返回 `None`。
    #[must_use]
    pub fn stem(&self) -> Option<String> {
        self.inner
            .file_stem()
            .and_then(|value| value.to_str())
            .map(ToOwned::to_owned)
    }

    /// 返回绝对路径。
    ///
    /// 这个方法不要求路径已经存在；相对路径会基于当前工作目录转换。
    pub fn absolute(&self) -> Result<Path> {
        absolute(self)
    }

    /// 对当前路径加阻塞独占锁。
    ///
    /// 锁文件会自动创建。返回的 [`Lock`] 在调用 `unlock()` 或离开作用域时释放锁。
    pub fn lock(&self) -> Result<Lock> {
        lock(self)
    }
}

/// 文件独占锁。
///
/// 由 [`lock`] 或 [`Path::lock`] 创建。这个类型只表示“当前进程持有某个锁文件”，不会暴露底层
/// `File`。需要提前释放时调用 [`unlock`](Self::unlock)，否则离开作用域时会尽力释放。
#[derive(Debug)]
pub struct Lock {
    path: Path,
    file: Option<std_fs::File>,
}

impl Lock {
    /// 返回锁文件路径。
    ///
    /// 适合写日志或调试，不会暴露底层文件句柄。
    #[must_use]
    pub fn path(&self) -> Path {
        self.path.clone()
    }

    /// 释放文件锁。
    ///
    /// 成功后会消费当前锁对象；释放失败时返回 [`ErrorKind::Lock`]。
    pub fn unlock(mut self) -> Result<()> {
        self.unlock_inner()
    }

    fn unlock_inner(&mut self) -> Result<()> {
        let Some(file) = self.file.take() else {
            return Ok(());
        };
        file.unlock().map_err(|source| {
            Error::with_source(
                ErrorKind::Lock {
                    operation: "unlock",
                    path: self.path.clone(),
                },
                source,
            )
        })
    }
}

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = self.unlock_inner();
    }
}

impl fmt::Display for Path {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.display())
    }
}

impl From<&str> for Path {
    fn from(path: &str) -> Self {
        Self {
            inner: PathBuf::from(path),
        }
    }
}

impl From<String> for Path {
    fn from(path: String) -> Self {
        Self {
            inner: PathBuf::from(path),
        }
    }
}

impl From<&String> for Path {
    fn from(path: &String) -> Self {
        Self {
            inner: PathBuf::from(path),
        }
    }
}

impl From<&Path> for Path {
    fn from(path: &Path) -> Self {
        path.clone()
    }
}

/// 创建一个高层路径对象。
///
/// 当你需要对同一路径连续做多个文件操作时，用这个函数会比反复传字符串更清晰。
#[must_use]
pub fn path(path: impl Into<Path>) -> Path {
    path.into()
}

/// 读取文件为 UTF-8 文本。
pub fn read_text(path: impl Into<Path>) -> Result<String> {
    let path = path.into();
    std_fs::read_to_string(&path.inner)
        .map_err(|source| Error::with_source(ErrorKind::Read { path }, source))
}

/// 读取文件为文本行。
///
/// 返回值不包含换行符；空行会保留为空字符串。无法读取文件时返回 [`ErrorKind::Read`]。
pub fn read_lines(path: impl Into<Path>) -> Result<Vec<String>> {
    Ok(read_text(path)?.lines().map(ToOwned::to_owned).collect())
}

/// 写入 UTF-8 文本，并自动创建父目录。
pub fn write_text(path: impl Into<Path>, text: impl AsRef<str>) -> Result<()> {
    write_bytes(path, text.as_ref().as_bytes())
}

/// 写入文本行，并自动创建父目录。
///
/// 每一行都会写入一个 `\n`，空列表会创建空文件。无法写入文件时返回 [`ErrorKind::Write`]。
pub fn write_lines<S>(path: impl Into<Path>, lines: impl IntoIterator<Item = S>) -> Result<()>
where
    S: AsRef<str>,
{
    let mut text = String::new();
    for line in lines {
        text.push_str(line.as_ref());
        text.push('\n');
    }
    write_text(path, text)
}

/// 读取文件为字节数组。
pub fn read_bytes(path: impl Into<Path>) -> Result<Vec<u8>> {
    let path = path.into();
    std_fs::read(&path.inner).map_err(|source| Error::with_source(ErrorKind::Read { path }, source))
}

/// 写入字节内容，并自动创建父目录。
pub fn write_bytes(path: impl Into<Path>, bytes: impl AsRef<[u8]>) -> Result<()> {
    let path = path.into();
    create_parent_dirs(&path)?;
    std_fs::write(&path.inner, bytes)
        .map_err(|source| Error::with_source(ErrorKind::Write { path }, source))
}

/// 读取 JSON 文件并解析为指定类型。
pub fn read_json<T>(path: impl Into<Path>) -> Result<T>
where
    T: DeserializeOwned,
{
    let path = path.into();
    let bytes = read_bytes(&path)?;
    serde_json::from_slice(&bytes)
        .map_err(|source| Error::with_source(ErrorKind::JsonDecode { path }, source))
}

/// 把值写成 pretty JSON，并自动创建父目录。
pub fn write_json<T>(path: impl Into<Path>, value: &T) -> Result<()>
where
    T: Serialize + ?Sized,
{
    let path = path.into();
    let bytes = serde_json::to_vec_pretty(value).map_err(|source| {
        Error::with_source(ErrorKind::JsonEncode { path: path.clone() }, source)
    })?;
    write_bytes(path, bytes)
}

/// 判断路径是否存在。
///
/// 这个函数不返回错误；权限不足等无法确认的情况按不存在处理。
#[must_use]
pub fn exists(path: impl Into<Path>) -> bool {
    path.into().inner.exists()
}

/// 创建目录及其所有父目录。
pub fn make_dir(path: impl Into<Path>) -> Result<()> {
    let path = path.into();
    std_fs::create_dir_all(&path.inner)
        .map_err(|source| Error::with_source(ErrorKind::MakeDir { path }, source))
}

/// 创建临时目录。
///
/// 返回高层 [`Path`]，可继续调用 `join`、`write_text`、`remove` 等文件操作方法。
pub fn temp_dir() -> Result<Path> {
    let root = std::env::temp_dir();
    temp_dir_in(&root)
}

fn temp_dir_in(root: &StdPath) -> Result<Path> {
    for _ in 0..TEMP_ATTEMPTS {
        let candidate = temp_candidate(root, "dir")?;
        match std_fs::create_dir(&candidate.inner) {
            Ok(()) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(temp_error_with_source(
                    "temp_dir",
                    candidate,
                    "create temporary directory failed",
                    source,
                ));
            }
        }
    }

    Err(temp_error(
        "temp_dir",
        Path::from_std_path(root),
        "too many temporary path collisions",
    ))
}

/// 创建临时文件。
///
/// 返回高层 [`Path`]，文件已存在且为空；可继续调用 `write_text`、`read_bytes`、`remove` 等方法。
pub fn temp_file() -> Result<Path> {
    let root = std::env::temp_dir();
    temp_file_in(&root)
}

fn temp_file_in(root: &StdPath) -> Result<Path> {
    for _ in 0..TEMP_ATTEMPTS {
        let candidate = temp_candidate(root, "file")?;
        match std_fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate.inner)
        {
            Ok(_) => return Ok(candidate),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(source) => {
                return Err(temp_error_with_source(
                    "temp_file",
                    candidate,
                    "create temporary file failed",
                    source,
                ));
            }
        }
    }

    Err(temp_error(
        "temp_file",
        Path::from_std_path(root),
        "too many temporary path collisions",
    ))
}

/// 列出目录下的直接子路径。
///
/// 返回结果按显示字符串排序，保证输出稳定。
pub fn list_dir(path: impl Into<Path>) -> Result<Vec<Path>> {
    let path = path.into();
    let mut entries = Vec::new();
    let dir = std_fs::read_dir(&path.inner)
        .map_err(|source| Error::with_source(ErrorKind::ListDir { path: path.clone() }, source))?;

    for entry in dir {
        let entry = entry.map_err(|source| {
            Error::with_source(ErrorKind::ListDir { path: path.clone() }, source)
        })?;
        entries.push(Path {
            inner: entry.path(),
        });
    }

    entries.sort_by_key(Path::display);
    Ok(entries)
}

/// 删除文件或目录。
///
/// 文件会直接删除，目录会递归删除；路径不存在时返回成功，方便脚本重复执行。
pub fn remove(path: impl Into<Path>) -> Result<()> {
    let path = path.into();
    let metadata = match std_fs::symlink_metadata(&path.inner) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => return Err(Error::with_source(ErrorKind::Remove { path }, source)),
    };

    let file_type = metadata.file_type();
    let result = if file_type.is_dir() && !file_type.is_symlink() {
        std_fs::remove_dir_all(&path.inner)
    } else {
        std_fs::remove_file(&path.inner)
    };

    result.map_err(|source| Error::with_source(ErrorKind::Remove { path }, source))
}

/// 复制文件或目录。
///
/// 文件复制会自动创建目标父目录；目录复制会递归复制内部内容。目标文件已存在时会覆盖。
pub fn copy(from: impl Into<Path>, to: impl Into<Path>) -> Result<()> {
    let from = from.into();
    let to = to.into();
    copy_inner(&from, &to)
}

/// 移动文件或目录。
///
/// 会自动创建目标父目录。非零散文件系统场景下使用系统 rename 语义。
pub fn move_path(from: impl Into<Path>, to: impl Into<Path>) -> Result<()> {
    let from = from.into();
    let to = to.into();
    create_parent_dirs(&to)?;
    std_fs::rename(&from.inner, &to.inner).map_err(|source| {
        Error::with_source(
            ErrorKind::Move {
                from: from.clone(),
                to: to.clone(),
            },
            source,
        )
    })
}

/// 递归列出目录下的所有子路径。
///
/// 返回结果不包含根路径本身，并按显示字符串排序，保证脚本输出和测试稳定。
pub fn walk(path: impl Into<Path>) -> Result<Vec<Path>> {
    let path = path.into();
    let mut output = Vec::new();
    walk_inner(&path, &mut output)?;
    output.sort_by_key(Path::display);
    Ok(output)
}

/// 返回文件大小，目录会返回其内部普通文件大小总和。
pub fn size(path: impl Into<Path>) -> Result<u64> {
    let path = path.into();
    size_inner(&path)
}

/// 返回绝对路径。
///
/// 这个函数不要求路径已经存在；相对路径会基于当前工作目录转换。
pub fn absolute(path: impl Into<Path>) -> Result<Path> {
    let path = path.into();
    let full = if path.inner.is_absolute() {
        path.inner.clone()
    } else {
        std::env::current_dir()
            .map_err(|source| {
                Error::with_source(
                    ErrorKind::Info {
                        operation: "absolute",
                        path: path.clone(),
                    },
                    source,
                )
            })?
            .join(&path.inner)
    };
    Ok(Path {
        inner: normalize_path(full),
    })
}

/// 对路径加阻塞独占锁。
///
/// 锁文件不存在时会自动创建，父目录也会自动创建。返回的 [`Lock`] 离开作用域时会自动释放锁。
pub fn lock(path: impl Into<Path>) -> Result<Lock> {
    let path = path.into();
    create_parent_dirs(&path)?;
    let file = std_fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path.inner)
        .map_err(|source| {
            Error::with_source(
                ErrorKind::Lock {
                    operation: "lock",
                    path: path.clone(),
                },
                source,
            )
        })?;
    file.lock().map_err(|source| {
        Error::with_source(
            ErrorKind::Lock {
                operation: "lock",
                path: path.clone(),
            },
            source,
        )
    })?;
    Ok(Lock {
        path,
        file: Some(file),
    })
}

fn copy_inner(from: &Path, to: &Path) -> Result<()> {
    let metadata = std_fs::symlink_metadata(&from.inner).map_err(|source| {
        Error::with_source(
            ErrorKind::Copy {
                from: from.clone(),
                to: to.clone(),
            },
            source,
        )
    })?;

    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        std_fs::create_dir_all(&to.inner).map_err(|source| {
            Error::with_source(
                ErrorKind::Copy {
                    from: from.clone(),
                    to: to.clone(),
                },
                source,
            )
        })?;

        let entries = std_fs::read_dir(&from.inner).map_err(|source| {
            Error::with_source(
                ErrorKind::Copy {
                    from: from.clone(),
                    to: to.clone(),
                },
                source,
            )
        })?;

        for entry in entries {
            let entry = entry.map_err(|source| {
                Error::with_source(
                    ErrorKind::Copy {
                        from: from.clone(),
                        to: to.clone(),
                    },
                    source,
                )
            })?;
            let child_from = Path {
                inner: entry.path(),
            };
            let child_to = Path {
                inner: to.inner.join(entry.file_name()),
            };
            copy_inner(&child_from, &child_to)?;
        }
        return Ok(());
    }

    create_parent_dirs(to)?;
    std_fs::copy(&from.inner, &to.inner)
        .map(|_| ())
        .map_err(|source| {
            Error::with_source(
                ErrorKind::Copy {
                    from: from.clone(),
                    to: to.clone(),
                },
                source,
            )
        })
}

fn walk_inner(path: &Path, output: &mut Vec<Path>) -> Result<()> {
    let entries = std_fs::read_dir(&path.inner)
        .map_err(|source| Error::with_source(ErrorKind::Walk { path: path.clone() }, source))?;

    for entry in entries {
        let entry = entry
            .map_err(|source| Error::with_source(ErrorKind::Walk { path: path.clone() }, source))?;
        let child = Path {
            inner: entry.path(),
        };
        output.push(child.clone());
        let metadata = std_fs::symlink_metadata(&child.inner).map_err(|source| {
            Error::with_source(
                ErrorKind::Walk {
                    path: child.clone(),
                },
                source,
            )
        })?;
        if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
            walk_inner(&child, output)?;
        }
    }

    Ok(())
}

fn size_inner(path: &Path) -> Result<u64> {
    let metadata = std_fs::symlink_metadata(&path.inner).map_err(|source| {
        Error::with_source(
            ErrorKind::Info {
                operation: "size",
                path: path.clone(),
            },
            source,
        )
    })?;

    if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
        let mut total = 0_u64;
        let entries = std_fs::read_dir(&path.inner).map_err(|source| {
            Error::with_source(
                ErrorKind::Info {
                    operation: "size",
                    path: path.clone(),
                },
                source,
            )
        })?;
        for entry in entries {
            let entry = entry.map_err(|source| {
                Error::with_source(
                    ErrorKind::Info {
                        operation: "size",
                        path: path.clone(),
                    },
                    source,
                )
            })?;
            total = total.saturating_add(size_inner(&Path {
                inner: entry.path(),
            })?);
        }
        return Ok(total);
    }

    Ok(metadata.len())
}

fn normalize_path(path: PathBuf) -> PathBuf {
    let mut output = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                output.pop();
            }
            other => output.push(other.as_os_str()),
        }
    }
    output
}

fn create_parent_dirs(path: &Path) -> Result<()> {
    let Some(parent) = path.inner.parent() else {
        return Ok(());
    };

    if parent.as_os_str().is_empty() {
        return Ok(());
    }

    std_fs::create_dir_all(parent).map_err(|source| {
        Error::with_source(
            ErrorKind::MakeDir {
                path: Path::from_std_path(parent),
            },
            source,
        )
    })
}

fn temp_candidate(root: &StdPath, kind: &str) -> Result<Path> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|source| {
        temp_error(
            temp_operation(kind),
            Path::from_std_path(root),
            &format!("system random source failed: {source}"),
        )
    })?;

    let suffix = temp_hex(&bytes);
    Ok(Path::from_std_path(&root.join(format!(
        "easy-rust-{kind}-{}-{suffix}",
        std::process::id()
    ))))
}

fn temp_operation(kind: &str) -> &'static str {
    if kind == "dir" {
        "temp_dir"
    } else {
        "temp_file"
    }
}

fn temp_error(operation: &'static str, path: Path, message: &str) -> Error {
    ErrorKind::Temp {
        operation,
        path,
        message: message.to_owned(),
    }
    .into()
}

fn temp_error_with_source(
    operation: &'static str,
    path: Path,
    message: &str,
    source: impl StdError + 'static,
) -> Error {
    Error::with_source(
        ErrorKind::Temp {
            operation,
            path,
            message: message.to_owned(),
        },
        source,
    )
}

fn temp_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
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
        fs as std_fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct User {
        id: u64,
        name: String,
    }

    fn temp_root(test_name: &str) -> std::result::Result<PathBuf, Box<dyn StdError>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!(
            "easy-rust-fs-{}-{test_name}-{nanos}",
            std::process::id()
        ));
        std_fs::create_dir_all(&root)?;
        Ok(root)
    }

    fn path_text(path: &std::path::Path) -> String {
        path.display().to_string()
    }

    #[test]
    fn write_text_creates_parent_dirs_and_reads_back() -> std::result::Result<(), Box<dyn StdError>>
    {
        let root = temp_root("text")?;
        let file = root.join("a/b/hello.txt");

        write_text(path_text(&file), "hello")?;

        assert_eq!(read_text(path_text(&file))?, "hello");
        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn write_bytes_creates_parent_dirs_and_reads_back() -> std::result::Result<(), Box<dyn StdError>>
    {
        let root = temp_root("bytes")?;
        let file = root.join("bin/data.bin");

        write_bytes(path_text(&file), [1_u8, 2, 3])?;

        assert_eq!(read_bytes(path_text(&file))?, vec![1, 2, 3]);
        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn read_lines_and_write_lines_use_simple_line_semantics()
    -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("lines")?;
        let file = path(path_text(&root)).join("nested/lines.txt");

        file.write_lines(["one", "", "three"])?;
        assert_eq!(
            file.read_lines()?,
            vec!["one".to_owned(), String::new(), "three".to_owned()]
        );

        let no_tail = path(path_text(&root)).join("no-tail.txt");
        no_tail.write_text("alpha\nbeta")?;
        assert_eq!(
            read_lines(&no_tail)?,
            vec!["alpha".to_owned(), "beta".to_owned()]
        );

        let empty = path(path_text(&root)).join("empty.txt");
        write_lines(&empty, Vec::<String>::new())?;
        assert_eq!(empty.read_text()?, "");

        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn write_json_is_pretty_and_read_json_decodes() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("json")?;
        let file = root.join("data/user.json");
        let user = User {
            id: 1,
            name: "Ada".to_owned(),
        };

        write_json(path_text(&file), &user)?;

        let text = read_text(path_text(&file))?;
        assert!(text.contains('\n'));
        assert!(text.contains("  \"id\""));
        assert_eq!(read_json::<User>(path_text(&file))?, user);
        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn invalid_json_returns_json_decode_error() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("invalid-json")?;
        let file = root.join("bad.json");
        write_text(path_text(&file), "{bad json")?;

        let error = match read_json::<User>(path_text(&file)) {
            Ok(_) => return Err("expected JSON decode error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::JsonDecode { path, .. } => {
                assert_eq!(path.display(), file.display().to_string())
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn path_object_supports_join_with_name_and_name() -> std::result::Result<(), Box<dyn StdError>>
    {
        let root = temp_root("path-object")?;
        let base = path(path_text(&root));
        let file = base.join("a").join("b.txt");

        file.write_text("x")?;
        let renamed = file.with_name("c.txt");
        renamed.write_text(file.read_text()?)?;

        assert_eq!(renamed.name().as_deref(), Some("c.txt"));
        assert_eq!(renamed.read_text()?, "x");
        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn make_dir_and_list_dir_return_sorted_paths() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("list-dir")?;
        let dir = path(path_text(&root)).join("items");
        dir.make_dir()?;
        dir.join("b.txt").write_text("b")?;
        dir.join("a.txt").write_text("a")?;

        let names: Vec<String> = dir
            .list_dir()?
            .into_iter()
            .filter_map(|path| path.name())
            .collect();

        assert_eq!(names, vec!["a.txt", "b.txt"]);
        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn make_dir_failure_uses_new_operation_name() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("make-dir-error")?;
        let file = root.join("file");
        std_fs::write(&file, "not a directory")?;
        let child = path(path_text(&file)).join("child");

        let error = match child.make_dir() {
            Ok(()) => return Err("expected make_dir error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::MakeDir { path } => assert!(path.display().contains("child")),
            other => return Err(format!("unexpected error: {other}").into()),
        }

        assert!(error.to_string().contains("make_dir"));
        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn list_dir_failure_uses_new_operation_name() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("list-dir-error")?;
        let file = root.join("file.txt");
        std_fs::write(&file, "not a directory")?;

        let error = match list_dir(path_text(&file)) {
            Ok(paths) => return Err(format!("expected list_dir error, got {paths:?}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::ListDir { path } => assert_eq!(path.display(), file.display().to_string()),
            other => return Err(format!("unexpected error: {other}").into()),
        }

        assert!(error.to_string().contains("list_dir"));
        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn copy_move_walk_size_and_path_info_work() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("copy-move-walk")?;
        let src = path(path_text(&root)).join("src");
        src.join("nested/a.txt").write_text("hello")?;
        src.join("b.bin").write_bytes([1_u8, 2, 3])?;
        let copied = path(path_text(&root)).join("copied");
        let moved = path(path_text(&root)).join("moved");

        copy(&src, &copied)?;
        copied.move_to(&moved)?;

        assert_eq!(moved.join("nested/a.txt").read_text()?, "hello");
        assert_eq!(moved.size()?, 8);
        let names: Vec<String> = moved
            .walk()?
            .into_iter()
            .filter_map(|path| path.name())
            .collect();
        assert_eq!(names, vec!["b.bin", "nested", "a.txt"]);

        let file = moved.join("nested/a.txt");
        assert_eq!(file.ext().as_deref(), Some("txt"));
        assert_eq!(file.stem().as_deref(), Some("a"));
        assert!(file.parent().is_some());
        assert!(file.absolute()?.display().contains("moved"));
        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn copy_missing_source_returns_context_error() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("copy-error")?;
        let from = path(path_text(&root)).join("missing");
        let to = path(path_text(&root)).join("to");

        let error = match copy(&from, &to) {
            Ok(()) => return Err("expected copy error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Copy {
                from: actual,
                to: target,
            } => {
                assert_eq!(actual.display(), from.display());
                assert_eq!(target.display(), to.display());
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }
        assert!(error.to_string().contains("copy"));
        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn remove_handles_file_dir_and_missing_path() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("remove")?;
        let file = path(path_text(&root)).join("nested/file.txt");
        file.write_text("delete me")?;

        assert!(exists(path_text(&root)));
        file.remove()?;
        assert!(!file.exists());

        write_text(path_text(&root.join("nested/again.txt")), "delete tree")?;
        remove(path_text(&root))?;
        assert!(!exists(path_text(&root)));

        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn temp_dir_creates_directory_path() -> std::result::Result<(), Box<dyn StdError>> {
        let dir = temp_dir()?;

        assert!(dir.exists());
        dir.join("hello.txt").write_text("hello")?;
        assert_eq!(dir.join("hello.txt").read_text()?, "hello");
        dir.remove()?;
        Ok(())
    }

    #[test]
    fn temp_file_creates_empty_file_path() -> std::result::Result<(), Box<dyn StdError>> {
        let file = temp_file()?;

        assert!(file.exists());
        assert_eq!(file.read_bytes()?, Vec::<u8>::new());
        file.write_text("hello")?;
        assert_eq!(file.read_text()?, "hello");
        file.remove()?;
        Ok(())
    }

    #[test]
    fn lock_creates_file_and_can_unlock_explicitly() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("lock")?;
        let lock_path = path(path_text(&root)).join("nested/app.lock");

        let guard = lock(&lock_path)?;
        assert!(lock_path.exists());
        assert_eq!(guard.path().display(), lock_path.display());
        guard.unlock()?;

        let guard = lock_path.lock()?;
        drop(guard);
        lock_path.remove()?;
        assert!(!lock_path.exists());
        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn temp_dir_create_failure_returns_temp_error() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("temp-dir-error")?;
        let not_a_dir = root.join("not-a-dir");
        std_fs::write(&not_a_dir, "file")?;

        let error = match temp_dir_in(&not_a_dir) {
            Ok(path) => return Err(format!("expected temp_dir error, got {path}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Temp {
                operation,
                path,
                message,
            } => {
                assert_eq!(*operation, "temp_dir");
                assert!(path.display().contains("not-a-dir"));
                assert_eq!(message, "create temporary directory failed");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        assert!(error.to_string().contains("temp_dir"));
        assert!(error.to_string().contains("not-a-dir"));
        assert!(error.source().is_some());
        remove(path_text(&root))?;
        Ok(())
    }

    #[test]
    fn temp_file_create_failure_returns_temp_error() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("temp-file-error")?;
        let not_a_dir = root.join("not-a-dir");
        std_fs::write(&not_a_dir, "file")?;

        let error = match temp_file_in(&not_a_dir) {
            Ok(path) => return Err(format!("expected temp_file error, got {path}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Temp {
                operation,
                path,
                message,
            } => {
                assert_eq!(*operation, "temp_file");
                assert!(path.display().contains("not-a-dir"));
                assert_eq!(message, "create temporary file failed");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        assert!(error.to_string().contains("temp_file"));
        assert!(error.to_string().contains("not-a-dir"));
        assert!(error.source().is_some());
        remove(path_text(&root))?;
        Ok(())
    }
}
