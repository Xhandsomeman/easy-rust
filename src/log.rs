//! 极简日志 API。
//!
//! 这个模块提供零配置日志：普通用户不需要初始化 logger、subscriber 或 formatter，直接调用
//! [`info`]、[`warn`]、[`error`] 就能输出日志。默认输出到 `stderr`，默认级别为 `info`。

use std::{
    error::Error as StdError,
    fmt,
    fs::{self as std_fs, File, OpenOptions},
    io::{self, Write},
    sync::{Mutex, MutexGuard, OnceLock},
};

use chrono::Local;

use crate::fs::Path as FsPath;

const TIMESTAMP_FORMAT: &str = "%Y-%m-%d %H:%M:%S";

/// log 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。当前主要用于配置日志级别和日志文件。
pub type Result<T> = std::result::Result<T, Error>;

/// log 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分级别错误、目录创建错误或文件打开错误时，
/// 使用 [`Error::kind`]。
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

/// log 模块的具体错误原因。
///
/// 错误信息会包含操作名、日志级别或路径，方便定位日志配置失败的位置。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 日志级别不合法。
    #[error("log level `{level}` failed: invalid level")]
    InvalidLevel {
        /// 调用方传入的日志级别。
        level: String,
    },

    /// 打开日志文件失败。
    #[error("log file `{path}` failed")]
    OpenFile {
        /// 发生错误的路径。
        path: String,
    },

    /// 创建日志文件父目录失败。
    #[error("log create_dir `{path}` failed")]
    CreateDir {
        /// 发生错误的目录路径。
        path: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Level {
    Debug,
    Info,
    Warn,
    Error,
}

impl Level {
    fn parse(value: &str) -> Result<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "debug" => Ok(Self::Debug),
            "info" => Ok(Self::Info),
            "warn" => Ok(Self::Warn),
            "error" => Ok(Self::Error),
            _ => Err(ErrorKind::InvalidLevel {
                level: value.to_owned(),
            }
            .into()),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Debug => "DEBUG",
            Self::Info => "INFO",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

enum Output {
    Stderr,
    File {
        path: String,
        file: File,
    },
    #[cfg(test)]
    Memory(std::sync::Arc<Mutex<Vec<String>>>),
}

struct State {
    level: Level,
    output: Output,
}

impl Default for State {
    fn default() -> Self {
        Self {
            level: Level::Info,
            output: Output::Stderr,
        }
    }
}

static STATE: OnceLock<Mutex<State>> = OnceLock::new();

/// 输出 debug 级别日志。
///
/// 默认级别为 `info`，所以未调用 `level("debug")` 前这条日志会被过滤。日志写入失败时不会
/// panic；文件写入失败会回退到 `stderr`。
pub fn debug(message: impl AsRef<str>) {
    write_log(Level::Debug, message.as_ref());
}

/// 输出 info 级别日志。
///
/// 适合记录服务启动、关键流程完成等普通运行信息。
pub fn info(message: impl AsRef<str>) {
    write_log(Level::Info, message.as_ref());
}

/// 输出 warn 级别日志。
///
/// 适合记录可恢复错误、重试、配置降级等需要关注但不一定中断程序的问题。
pub fn warn(message: impl AsRef<str>) {
    write_log(Level::Warn, message.as_ref());
}

/// 输出 error 级别日志。
///
/// 适合记录请求失败、任务失败等需要明确排查的问题。这个函数只负责写日志，不会终止程序。
pub fn error(message: impl AsRef<str>) {
    write_log(Level::Error, message.as_ref());
}

/// 设置全局日志级别。
///
/// 支持 `debug`、`info`、`warn`、`error`，大小写不敏感。允许重复调用，后续日志
/// 会立即使用新级别。
pub fn level(level: impl AsRef<str>) -> Result<()> {
    let level = Level::parse(level.as_ref())?;
    lock_state().level = level;
    Ok(())
}

/// 把后续日志切换到文件输出。
///
/// 文件使用追加写入，会自动创建父目录。允许重复调用，后续日志会切换到新文件；调用后日志
/// 只写文件，不再写 `stderr`。
pub fn file(path: impl Into<FsPath>) -> Result<()> {
    let path = path.into();
    let std_path = path.as_std_path();

    if let Some(parent) = std_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std_fs::create_dir_all(parent).map_err(|source| {
            Error::with_source(
                ErrorKind::CreateDir {
                    path: parent.display().to_string(),
                },
                source,
            )
        })?;
    }

    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(std_path)
        .map_err(|source| {
            Error::with_source(
                ErrorKind::OpenFile {
                    path: path.display(),
                },
                source,
            )
        })?;

    lock_state().output = Output::File {
        path: path.display(),
        file,
    };
    Ok(())
}

fn state() -> &'static Mutex<State> {
    STATE.get_or_init(|| Mutex::new(State::default()))
}

fn lock_state() -> MutexGuard<'static, State> {
    match state().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn write_log(level: Level, message: &str) {
    let mut state = lock_state();

    if level < state.level {
        return;
    }

    let line = format_line(level, message);
    write_output(&mut state.output, &line);
}

fn format_line(level: Level, message: &str) -> String {
    format!(
        "{} {} {}",
        Local::now().format(TIMESTAMP_FORMAT),
        level.label(),
        message
    )
}

fn write_output(output: &mut Output, line: &str) {
    match output {
        Output::Stderr => {
            let _ = writeln!(io::stderr().lock(), "{line}");
        }
        Output::File { path, file } => {
            if let Err(error) = writeln!(file, "{line}") {
                fallback_to_stderr(path, error, line);
            }
        }
        #[cfg(test)]
        Output::Memory(lines) => {
            let mut lines = match lines.lock() {
                Ok(lines) => lines,
                Err(poisoned) => poisoned.into_inner(),
            };
            lines.push(line.to_owned());
        }
    }
}

fn fallback_to_stderr(path: &str, error: io::Error, line: &str) {
    let mut stderr = io::stderr().lock();
    let _ = writeln!(
        stderr,
        "{} ERROR log file `{path}` write failed: {error}",
        Local::now().format(TIMESTAMP_FORMAT)
    );
    let _ = writeln!(stderr, "{line}");
}

#[cfg(test)]
fn reset_for_tests() -> std::sync::Arc<Mutex<Vec<String>>> {
    let lines = std::sync::Arc::new(Mutex::new(Vec::new()));
    let mut state = lock_state();
    state.level = Level::Info;
    state.output = Output::Memory(lines.clone());
    lines
}

#[cfg(test)]
fn captured_lines(lines: &std::sync::Arc<Mutex<Vec<String>>>) -> Vec<String> {
    match lines.lock() {
        Ok(lines) => lines.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error as StdError,
        fs as test_fs,
        path::PathBuf,
        sync::Mutex,
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn test_guard() -> std::result::Result<std::sync::MutexGuard<'static, ()>, Box<dyn StdError>> {
        TEST_LOCK
            .lock()
            .map_err(|_| "log test lock poisoned".into())
    }

    fn temp_root(test_name: &str) -> std::result::Result<PathBuf, Box<dyn StdError>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!(
            "easy-rust-log-{}-{test_name}-{nanos}",
            std::process::id()
        ));
        test_fs::create_dir_all(&root)?;
        Ok(root)
    }

    fn path_text(path: &std::path::Path) -> String {
        path.display().to_string()
    }

    #[test]
    fn default_level_filters_debug_and_writes_info_warn_error()
    -> std::result::Result<(), Box<dyn StdError>> {
        let _guard = test_guard()?;
        let lines = reset_for_tests();

        debug("hidden");
        info("service started");
        warn("retrying");
        error("failed");

        let lines = captured_lines(&lines);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains(" INFO service started"));
        assert!(lines[1].contains(" WARN retrying"));
        assert!(lines[2].contains(" ERROR failed"));
        Ok(())
    }

    #[test]
    fn debug_level_enables_debug_logs() -> std::result::Result<(), Box<dyn StdError>> {
        let _guard = test_guard()?;
        let lines = reset_for_tests();

        level("debug")?;
        debug("visible");

        let lines = captured_lines(&lines);
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains(" DEBUG visible"));
        Ok(())
    }

    #[test]
    fn warning_alias_is_rejected() -> std::result::Result<(), Box<dyn StdError>> {
        let _guard = test_guard()?;
        reset_for_tests();

        let error = match level("warning") {
            Ok(()) => return Err("expected invalid level error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::InvalidLevel { level } => assert_eq!(level, "warning"),
            other => return Err(format!("unexpected error: {other}").into()),
        }
        Ok(())
    }

    #[test]
    fn invalid_level_returns_error() -> std::result::Result<(), Box<dyn StdError>> {
        let _guard = test_guard()?;
        let _lines = reset_for_tests();

        let error = match level("verbose") {
            Ok(()) => return Err("expected invalid level error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::InvalidLevel { level } => assert_eq!(level, "verbose"),
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[test]
    fn file_creates_parent_dirs_and_appends_logs() -> std::result::Result<(), Box<dyn StdError>> {
        let _guard = test_guard()?;
        let _lines = reset_for_tests();
        let root = temp_root("file")?;
        let path = root.join("nested/app.log");

        file(path_text(&path))?;
        info("first");
        info("second");

        let text = test_fs::read_to_string(&path)?;
        assert!(text.contains(" INFO first"));
        assert!(text.contains(" INFO second"));

        file(path_text(&path))?;
        warn("third");

        let text = test_fs::read_to_string(&path)?;
        assert!(text.contains(" WARN third"));
        assert!(text.lines().count() >= 3);
        Ok(())
    }

    #[test]
    fn repeated_file_calls_switch_output() -> std::result::Result<(), Box<dyn StdError>> {
        let _guard = test_guard()?;
        let _lines = reset_for_tests();
        let root = temp_root("switch")?;
        let first = root.join("first.log");
        let second = root.join("second.log");

        file(path_text(&first))?;
        info("first only");
        file(path_text(&second))?;
        info("second only");

        let first_text = test_fs::read_to_string(&first)?;
        let second_text = test_fs::read_to_string(&second)?;
        assert!(first_text.contains(" INFO first only"));
        assert!(!first_text.contains(" INFO second only"));
        assert!(second_text.contains(" INFO second only"));
        Ok(())
    }

    #[test]
    fn log_line_contains_timestamp_level_and_message() -> std::result::Result<(), Box<dyn StdError>>
    {
        let _guard = test_guard()?;
        let lines = reset_for_tests();

        info("hello");

        let lines = captured_lines(&lines);
        let line = lines.first().ok_or("missing log line")?;
        assert!(line.len() >= "YYYY-MM-DD HH:MM:SS INFO hello".len());
        assert_eq!(&line[4..5], "-");
        assert_eq!(&line[13..14], ":");
        assert!(line.contains(" INFO hello"));
        Ok(())
    }

    #[test]
    fn concurrent_logging_keeps_one_line_per_message() -> std::result::Result<(), Box<dyn StdError>>
    {
        let _guard = test_guard()?;
        let lines = reset_for_tests();
        level("debug")?;

        let handles: Vec<_> = (0..8)
            .map(|index| thread::spawn(move || info(format!("message-{index}"))))
            .collect();

        for handle in handles {
            handle.join().map_err(|_| "thread panicked")?;
        }

        let lines = captured_lines(&lines);
        assert_eq!(lines.len(), 8);
        assert!(lines.iter().all(|line| line.contains(" INFO message-")));
        Ok(())
    }
}
