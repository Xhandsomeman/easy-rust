//! 极简正则 API。
//!
//! 这个模块提供常见正则匹配、查找、替换和分割入口。调用方只传 pattern 和文本，
//! 不需要先创建可复用对象。

use std::{error::Error as StdError, fmt};

/// regex 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。常见写法是 `regex::is_match(pattern, text)?`。
pub type Result<T> = std::result::Result<T, Error>;

/// regex 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要定位正则语法错误时，使用 [`Error::kind`]。
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

/// regex 模块的具体错误原因。
///
/// 错误信息会包含操作名和 pattern，方便定位哪个正则表达式无法编译。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 正则表达式编译失败。
    #[error("regex {operation} pattern `{pattern}` failed")]
    Pattern {
        /// 发生错误的操作名，例如 `is_match`。
        operation: &'static str,
        /// 用户传入的正则表达式。
        pattern: String,
    },
}

/// 可重复使用的高层正则对象。
///
/// 使用 [`new`] 创建。适合在循环或服务热路径里复用同一个 pattern，避免每次调用短入口都重新
/// 编译正则。这个类型不暴露底层正则库类型。
#[derive(Clone, Debug)]
pub struct Regex {
    inner: regex_crate::Regex,
}

impl Regex {
    /// 判断文本是否匹配当前正则。
    #[must_use]
    pub fn is_match(&self, text: impl AsRef<str>) -> bool {
        self.inner.is_match(text.as_ref())
    }

    /// 查找第一个匹配文本。
    ///
    /// 没有匹配时返回 `None`。
    #[must_use]
    pub fn find(&self, text: impl AsRef<str>) -> Option<String> {
        self.inner
            .find(text.as_ref())
            .map(|matched| matched.as_str().to_owned())
    }

    /// 查找全部匹配文本。
    ///
    /// 没有匹配时返回空列表；结果按文本中出现顺序返回。
    #[must_use]
    pub fn find_all(&self, text: impl AsRef<str>) -> Vec<String> {
        self.inner
            .find_iter(text.as_ref())
            .map(|matched| matched.as_str().to_owned())
            .collect()
    }

    /// 替换全部匹配文本。
    ///
    /// replacement 使用正则替换字符串语义，例如 `$1` 表示第一个捕获组。
    #[must_use]
    pub fn replace(&self, text: impl AsRef<str>, replacement: impl AsRef<str>) -> String {
        self.inner
            .replace_all(text.as_ref(), replacement.as_ref())
            .into_owned()
    }

    /// 按正则表达式分割文本。
    ///
    /// 返回值保留空片段，方便调用方按自己的规则处理。
    #[must_use]
    pub fn split(&self, text: impl AsRef<str>) -> Vec<String> {
        self.inner
            .split(text.as_ref())
            .map(ToOwned::to_owned)
            .collect()
    }
}

/// 编译可重复使用的正则对象。
///
/// 适合循环或服务热路径。pattern 非法时返回 [`ErrorKind::Pattern`]。
pub fn new(pattern: impl AsRef<str>) -> Result<Regex> {
    compile("new", pattern.as_ref()).map(|inner| Regex { inner })
}

/// 判断文本是否匹配正则表达式。
///
/// pattern 非法时返回 [`ErrorKind::Pattern`]。
pub fn is_match(pattern: impl AsRef<str>, text: impl AsRef<str>) -> Result<bool> {
    Ok(compile("is_match", pattern.as_ref())?.is_match(text.as_ref()))
}

/// 查找第一个匹配文本。
///
/// 没有匹配时返回 `Ok(None)`；pattern 非法时返回 [`ErrorKind::Pattern`]。
pub fn find(pattern: impl AsRef<str>, text: impl AsRef<str>) -> Result<Option<String>> {
    Ok(compile("find", pattern.as_ref())?
        .find(text.as_ref())
        .map(|matched| matched.as_str().to_owned()))
}

/// 查找全部匹配文本。
///
/// 没有匹配时返回空列表；结果按文本中出现顺序返回。
pub fn find_all(pattern: impl AsRef<str>, text: impl AsRef<str>) -> Result<Vec<String>> {
    Ok(compile("find_all", pattern.as_ref())?
        .find_iter(text.as_ref())
        .map(|matched| matched.as_str().to_owned())
        .collect())
}

/// 替换全部匹配文本。
///
/// replacement 使用正则替换字符串语义，例如 `$1` 表示第一个捕获组。
pub fn replace(
    pattern: impl AsRef<str>,
    text: impl AsRef<str>,
    replacement: impl AsRef<str>,
) -> Result<String> {
    Ok(compile("replace", pattern.as_ref())?
        .replace_all(text.as_ref(), replacement.as_ref())
        .into_owned())
}

/// 按正则表达式分割文本。
///
/// 返回值保留空片段，方便调用方按自己的规则处理。
pub fn split(pattern: impl AsRef<str>, text: impl AsRef<str>) -> Result<Vec<String>> {
    Ok(compile("split", pattern.as_ref())?
        .split(text.as_ref())
        .map(ToOwned::to_owned)
        .collect())
}

fn compile(operation: &'static str, pattern: &str) -> Result<regex_crate::Regex> {
    regex_crate::Regex::new(pattern).map_err(|source| {
        Error::with_source(
            ErrorKind::Pattern {
                operation,
                pattern: pattern.to_owned(),
            },
            source,
        )
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    #[test]
    fn matching_and_finding_work() -> std::result::Result<(), Box<dyn StdError>> {
        assert!(is_match(r"\d+", "room 42")?);
        assert_eq!(find(r"\d+", "room 42")?, Some("42".to_owned()));
        assert_eq!(find(r"\d+", "room")?, None);
        assert_eq!(
            find_all(r"\d+", "a1 b22 c333")?,
            vec!["1".to_owned(), "22".to_owned(), "333".to_owned()]
        );
        Ok(())
    }

    #[test]
    fn replace_and_split_work() -> std::result::Result<(), Box<dyn StdError>> {
        assert_eq!(replace(r"(\w+)=(\d+)", "id=42", "$1:$2")?, "id:42");
        assert_eq!(
            split(r"\s*,\s*", "a, b, c")?,
            vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]
        );
        Ok(())
    }

    #[test]
    fn compiled_regex_can_be_reused() -> std::result::Result<(), Box<dyn StdError>> {
        let regex = new(r"\d+")?;

        assert!(regex.is_match("room 42"));
        assert_eq!(regex.find("room 42"), Some("42".to_owned()));
        assert_eq!(
            regex.find_all("a1 b22"),
            vec!["1".to_owned(), "22".to_owned()]
        );
        assert_eq!(regex.replace("room 42", "#"), "room #");
        assert_eq!(
            new(r"\s*,\s*")?.split("a, b, c"),
            vec!["a".to_owned(), "b".to_owned(), "c".to_owned()]
        );
        Ok(())
    }

    #[test]
    fn invalid_pattern_returns_context_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match is_match("[", "text") {
            Ok(value) => return Err(format!("expected pattern error, got {value}").into()),
            Err(error) => error,
        };

        assert!(error.to_string().contains("is_match"));
        assert!(error.to_string().contains("["));

        let new_error = match new("[") {
            Ok(value) => return Err(format!("expected pattern error, got {value:?}").into()),
            Err(error) => error,
        };
        assert!(new_error.to_string().contains("new"));
        Ok(())
    }
}
