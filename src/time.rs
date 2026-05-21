//! 极简时间 API。
//!
//! 这个模块提供脚本和后端最常用的时间能力：当前时间、今天日期、Unix 时间戳、
//! 格式化、解析和同步 sleep。底层时间库只作为内部实现，不作为主路径暴露给用户。

use std::{error::Error as StdError, fmt, thread, time::Duration};

use chrono::{
    DateTime as ChronoDateTime, Local, LocalResult, NaiveDate, NaiveDateTime, TimeZone,
    format::{Item, StrftimeItems},
};

const DEFAULT_DATETIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S";
const DEFAULT_DATE_FORMAT: &str = "%Y-%m-%d";

/// time 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。常见写法是
/// `let value = time::parse("2026-05-19 12:00:00", "%Y-%m-%d %H:%M:%S")?;`。
pub type Result<T> = std::result::Result<T, Error>;

/// time 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分格式错误和解析错误时，使用
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

/// time 模块的具体错误原因。
///
/// 错误信息会包含操作名、输入文本或格式字符串，方便定位时间处理失败的位置。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 时间格式字符串不合法。
    #[error("time {operation} failed: invalid format `{pattern}`")]
    Format {
        /// 发生错误的操作名，例如 `format` 或 `parse`。
        operation: &'static str,
        /// 调用方传入的格式字符串。
        pattern: String,
    },

    /// 时间文本解析失败。
    #[error("time parse `{text}` with `{pattern}` failed")]
    Parse {
        /// 调用方传入的时间文本。
        text: String,
        /// 调用方传入的格式字符串。
        pattern: String,
    },

    /// 本地时间无法唯一确定。
    #[error("time parse `{text}` with `{pattern}` failed: local time is ambiguous or invalid")]
    Ambiguous {
        /// 调用方传入的时间文本。
        text: String,
        /// 调用方传入的格式字符串。
        pattern: String,
    },
}

/// easy-rust 的高层日期时间对象。
///
/// 这个类型表示本地时区下的某一刻。它只暴露脚本和后端常用方法，不暴露底层时间库类型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DateTime {
    inner: ChronoDateTime<Local>,
}

impl DateTime {
    /// 返回当前日期时间对应的 Unix 时间戳秒数。
    #[must_use]
    pub fn unix_time(&self) -> i64 {
        self.inner.timestamp()
    }

    /// 返回日期部分。
    ///
    /// 适合从完整日期时间中取出当天日期。
    #[must_use]
    pub fn date(&self) -> Date {
        Date {
            inner: self.inner.date_naive(),
        }
    }

    /// 按格式字符串输出日期时间。
    ///
    /// 格式规则使用常见 strftime 风格，例如 `%Y-%m-%d %H:%M:%S`。
    pub fn format(&self, pattern: impl AsRef<str>) -> Result<String> {
        format_inner(&self.inner, pattern.as_ref(), "format")
    }
}

impl fmt::Display for DateTime {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.inner.format(DEFAULT_DATETIME_FORMAT))
    }
}

/// easy-rust 的高层日期对象。
///
/// 这个类型表示本地日期，不包含时分秒。需要完整日期时间时，使用 [`now`] 或 [`parse`]。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Date {
    inner: NaiveDate,
}

impl Date {
    /// 按格式字符串输出日期。
    ///
    /// 格式规则使用常见 strftime 风格，例如 `%Y-%m-%d`。
    pub fn format(&self, pattern: impl AsRef<str>) -> Result<String> {
        let pattern = pattern.as_ref();
        validate_pattern("format", pattern)?;
        Ok(self.inner.format(pattern).to_string())
    }
}

impl fmt::Display for Date {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.inner.format(DEFAULT_DATE_FORMAT))
    }
}

/// 返回当前本地日期时间。
#[must_use]
pub fn now() -> DateTime {
    DateTime {
        inner: Local::now(),
    }
}

/// 返回今天的本地日期。
#[must_use]
pub fn today() -> Date {
    Date {
        inner: Local::now().date_naive(),
    }
}

/// 返回当前 Unix 时间戳秒数。
#[must_use]
pub fn unix_time() -> i64 {
    Local::now().timestamp()
}

/// 按格式字符串输出日期时间。
///
/// 这是 [`DateTime::format`] 的函数式入口，适合 `time::format(time::now(), "...")?`
/// 这种脚本式写法。
pub fn format(value: DateTime, pattern: impl AsRef<str>) -> Result<String> {
    value.format(pattern)
}

/// 按格式字符串解析本地日期时间。
///
/// 优先解析完整日期时间；如果格式只包含日期，会把时间部分设为 `00:00:00`。
pub fn parse(text: impl AsRef<str>, pattern: impl AsRef<str>) -> Result<DateTime> {
    let text = text.as_ref();
    let pattern = pattern.as_ref();
    validate_pattern("parse", pattern)?;

    let naive = match NaiveDateTime::parse_from_str(text, pattern) {
        Ok(value) => value,
        Err(datetime_error) => match NaiveDate::parse_from_str(text, pattern) {
            Ok(date) => date
                .and_hms_opt(0, 0, 0)
                .ok_or_else(|| ambiguous_error(text, pattern))?,
            Err(_) => {
                return Err(Error::with_source(
                    ErrorKind::Parse {
                        text: text.to_owned(),
                        pattern: pattern.to_owned(),
                    },
                    datetime_error,
                ));
            }
        },
    };

    match Local.from_local_datetime(&naive) {
        LocalResult::Single(value) => Ok(DateTime { inner: value }),
        LocalResult::Ambiguous(_, _) | LocalResult::None => Err(ambiguous_error(text, pattern)),
    }
}

/// 同步睡眠指定秒数。
///
/// 这个函数适合脚本和简单后端工具。异步代码中应继续使用运行时自己的 sleep。
pub fn sleep_seconds(seconds: u64) {
    thread::sleep(Duration::from_secs(seconds));
}

/// 异步睡眠指定毫秒数。
///
/// 这个函数适合 Tokio 异步代码，调用时使用 `time::sleep_ms_async(100).await`。
pub async fn sleep_ms_async(ms: u64) {
    tokio::time::sleep(Duration::from_millis(ms)).await;
}

/// 异步睡眠指定秒数。
///
/// 这个函数适合 Tokio 异步代码，调用时使用 `time::sleep_secs_async(1).await`。
pub async fn sleep_secs_async(seconds: u64) {
    tokio::time::sleep(Duration::from_secs(seconds)).await;
}

/// 创建秒级时间长度。
///
/// 适合给缓存 TTL、请求超时等 API 传入简单秒数，例如 `cache.set_ttl("token", value, time::seconds(60))`。
#[must_use]
pub fn seconds(seconds: u64) -> Duration {
    Duration::from_secs(seconds)
}

/// 创建毫秒级时间长度。
///
/// 适合需要比秒更细的超时、等待或测试场景。
#[must_use]
pub fn millis(millis: u64) -> Duration {
    Duration::from_millis(millis)
}

/// 创建分钟级时间长度。
///
/// 内部使用饱和乘法，极大输入会饱和到 `u64::MAX` 秒，避免溢出 panic。
#[must_use]
pub fn minutes(minutes: u64) -> Duration {
    Duration::from_secs(minutes.saturating_mul(60))
}

/// 创建小时级时间长度。
///
/// 内部使用饱和乘法，极大输入会饱和到 `u64::MAX` 秒，避免溢出 panic。
#[must_use]
pub fn hours(hours: u64) -> Duration {
    Duration::from_secs(hours.saturating_mul(60).saturating_mul(60))
}

fn format_inner(
    value: &ChronoDateTime<Local>,
    pattern: &str,
    operation: &'static str,
) -> Result<String> {
    validate_pattern(operation, pattern)?;
    Ok(value.format(pattern).to_string())
}

fn validate_pattern(operation: &'static str, pattern: &str) -> Result<()> {
    let invalid = StrftimeItems::new(pattern).any(|item| matches!(item, Item::Error));

    if invalid {
        return Err(ErrorKind::Format {
            operation,
            pattern: pattern.to_owned(),
        }
        .into());
    }

    Ok(())
}

fn ambiguous_error(text: &str, pattern: &str) -> Error {
    ErrorKind::Ambiguous {
        text: text.to_owned(),
        pattern: pattern.to_owned(),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    #[test]
    fn now_and_unix_time_return_current_timestamp() {
        let before = unix_time();
        let current = now();
        let after = unix_time();

        assert!(before > 0);
        assert!(current.unix_time() >= before);
        assert!(current.unix_time() <= after);
    }

    #[test]
    fn today_formats_date() -> std::result::Result<(), Box<dyn StdError>> {
        let text = today().format("%Y-%m-%d")?;

        assert_eq!(text.len(), 10);
        assert_eq!(&text[4..5], "-");
        assert_eq!(&text[7..8], "-");
        Ok(())
    }

    #[test]
    fn parse_and_format_datetime() -> std::result::Result<(), Box<dyn StdError>> {
        let value = parse("2026-05-19 12:34:56", "%Y-%m-%d %H:%M:%S")?;
        let text = format(value.clone(), "%Y-%m-%d %H:%M:%S")?;

        assert_eq!(text, "2026-05-19 12:34:56");
        assert_eq!(value.to_string(), "2026-05-19 12:34:56");
        Ok(())
    }

    #[test]
    fn parse_date_only_defaults_to_midnight() -> std::result::Result<(), Box<dyn StdError>> {
        let value = parse("2026-05-19", "%Y-%m-%d")?;

        assert_eq!(value.format("%Y-%m-%d %H:%M:%S")?, "2026-05-19 00:00:00");
        assert_eq!(value.date().to_string(), "2026-05-19");
        Ok(())
    }

    #[test]
    fn invalid_format_returns_format_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match now().format("%Q") {
            Ok(_) => return Err("expected format error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Format { operation, pattern } => {
                assert_eq!(*operation, "format");
                assert_eq!(pattern, "%Q");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[test]
    fn invalid_parse_returns_parse_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match parse("not a time", "%Y-%m-%d %H:%M:%S") {
            Ok(_) => return Err("expected parse error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Parse { text, pattern, .. } => {
                assert_eq!(text, "not a time");
                assert_eq!(pattern, "%Y-%m-%d %H:%M:%S");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[test]
    fn sleep_zero_seconds_returns() {
        sleep_seconds(0);
    }

    #[tokio::test]
    async fn async_sleep_helpers_return() {
        sleep_ms_async(0).await;
        sleep_secs_async(0).await;
    }

    #[test]
    fn seconds_returns_duration() {
        assert_eq!(seconds(60), Duration::from_secs(60));
    }

    #[test]
    fn simple_duration_helpers_return_expected_values() {
        assert_eq!(millis(250), Duration::from_millis(250));
        assert_eq!(minutes(2), Duration::from_secs(120));
        assert_eq!(hours(2), Duration::from_secs(7200));
        assert_eq!(minutes(u64::MAX), Duration::from_secs(u64::MAX));
        assert_eq!(hours(u64::MAX), Duration::from_secs(u64::MAX));
    }
}
