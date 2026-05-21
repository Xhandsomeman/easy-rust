//! 极简 UUID API。
//!
//! 这个模块提供 UUID v4 生成、解析规范化和合法性检查。默认生成标准带连字符的小写 UUID 字符串。

use std::{error::Error as StdError, fmt};

/// uuid 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。当前主要用于解析 UUID 字符串失败。
pub type Result<T> = std::result::Result<T, Error>;

/// uuid 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要定位 UUID 解析失败时，使用 [`Error::kind`]。
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

/// uuid 模块的具体错误原因。
///
/// 错误信息会包含操作名和输入文本，方便定位解析失败的位置。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// UUID 文本解析失败。
    #[error("uuid parse `{input}` failed")]
    Parse {
        /// 调用方传入的 UUID 文本。
        input: String,
    },
}

/// 生成 UUID v4 字符串。
///
/// 返回标准带连字符的小写格式，例如 `550e8400-e29b-41d4-a716-446655440000`。
#[must_use]
pub fn new() -> String {
    uuid_crate::Uuid::new_v4().to_string()
}

/// 解析 UUID 并返回规范化字符串。
///
/// 输入可以是大小写混合的 UUID；成功后返回标准带连字符的小写格式。
pub fn parse(input: impl AsRef<str>) -> Result<String> {
    let input = input.as_ref();
    uuid_crate::Uuid::parse_str(input)
        .map(|value| value.to_string())
        .map_err(|source| {
            Error::with_source(
                ErrorKind::Parse {
                    input: input.to_owned(),
                },
                source,
            )
        })
}

/// 判断文本是否是合法 UUID。
#[must_use]
pub fn is_valid(input: impl AsRef<str>) -> bool {
    uuid_crate::Uuid::parse_str(input.as_ref()).is_ok()
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    #[test]
    fn new_returns_valid_uuid() {
        let value = new();

        assert!(is_valid(&value));
        assert_eq!(value.len(), 36);
    }

    #[test]
    fn parse_normalizes_uuid() -> std::result::Result<(), Box<dyn StdError>> {
        let value = parse("550E8400-E29B-41D4-A716-446655440000")?;

        assert_eq!(value, "550e8400-e29b-41d4-a716-446655440000");
        Ok(())
    }

    #[test]
    fn invalid_uuid_returns_parse_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match parse("not-a-uuid") {
            Ok(value) => return Err(format!("expected parse error, got {value}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Parse { input, .. } => assert_eq!(input, "not-a-uuid"),
        }

        assert!(!is_valid("not-a-uuid"));
        Ok(())
    }
}
