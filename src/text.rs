//! 极简字符串处理 API。
//!
//! 这个模块提供脚本和后端最常用的字符串处理能力：清洗空白、截断、大小写转换、slug 和简单模板替换。

use std::{collections::HashMap, error::Error as StdError, fmt};

/// text 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。当前主要用于模板替换失败。
pub type Result<T> = std::result::Result<T, Error>;

/// text 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分缺失变量或模板格式错误时，使用 [`Error::kind`]。
#[derive(Debug)]
pub struct Error {
    kind: ErrorKind,
}

impl Error {
    fn new(kind: ErrorKind) -> Self {
        Self { kind }
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
        self.kind.fmt(formatter)
    }
}

impl StdError for Error {}

/// text 模块的具体错误原因。
///
/// 错误信息会包含操作名和变量名或模板说明，方便定位替换失败的位置。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 模板变量缺失。
    #[error("text {operation} failed: missing key `{key}`")]
    Missing {
        /// 发生错误的操作名，例如 `render`。
        operation: &'static str,
        /// 缺失的变量名。
        key: String,
    },

    /// 模板格式不合法。
    #[error("text {operation} failed: {message}")]
    Template {
        /// 发生错误的操作名，例如 `render`。
        operation: &'static str,
        /// 面向人的模板错误说明。
        message: String,
    },
}

/// 清洗字符串空白。
///
/// 会去掉首尾空白，并把中间连续空白压缩成单个空格，适合处理用户输入和日志文本。
#[must_use]
pub fn clean(text: impl AsRef<str>) -> String {
    text.as_ref()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// 按字符数量截断字符串。
///
/// `max_chars` 是 Unicode 字符数量，不会截断到 UTF-8 字节中间。长度未超过限制时返回原文本。
#[must_use]
pub fn truncate(text: impl AsRef<str>, max_chars: usize) -> String {
    text.as_ref().chars().take(max_chars).collect()
}

/// 转成小写。
///
/// 使用 Rust 标准 Unicode 小写规则。
#[must_use]
pub fn lower(text: impl AsRef<str>) -> String {
    text.as_ref().to_lowercase()
}

/// 转成大写。
///
/// 使用 Rust 标准 Unicode 大写规则。
#[must_use]
pub fn upper(text: impl AsRef<str>) -> String {
    text.as_ref().to_uppercase()
}

/// 转成标题样式。
///
/// 会先按空白拆词，再把每个词的首字符转大写，其余字符转小写。
#[must_use]
pub fn title(text: impl AsRef<str>) -> String {
    text.as_ref()
        .split_whitespace()
        .map(title_word)
        .collect::<Vec<_>>()
        .join(" ")
}

/// 生成 URL 友好的 slug。
///
/// 会转小写，保留 ASCII 字母数字，把其它连续字符压缩成一个 `-`，并去掉首尾 `-`。
#[must_use]
pub fn slug(text: impl AsRef<str>) -> String {
    let mut output = String::new();
    let mut last_was_dash = false;

    for character in text.as_ref().chars().flat_map(char::to_lowercase) {
        if character.is_ascii_alphanumeric() {
            output.push(character);
            last_was_dash = false;
        } else if !last_was_dash && !output.is_empty() {
            output.push('-');
            last_was_dash = true;
        }
    }

    output.trim_matches('-').to_owned()
}

/// 渲染简单模板。
///
/// 模板变量使用 `{name}` 格式，变量值从 `values` 中读取。缺失变量会返回 [`ErrorKind::Missing`]；
/// 未闭合 `{` 或空变量名会返回 [`ErrorKind::Template`]。
pub fn render<K, V>(
    template: impl AsRef<str>,
    values: impl IntoIterator<Item = (K, V)>,
) -> Result<String>
where
    K: AsRef<str>,
    V: ToString,
{
    let values: HashMap<String, String> = values
        .into_iter()
        .map(|(key, value)| (key.as_ref().to_owned(), value.to_string()))
        .collect();
    let template = template.as_ref();
    let mut output = String::new();
    let mut chars = template.chars().peekable();

    while let Some(character) = chars.next() {
        if character != '{' {
            output.push(character);
            continue;
        }

        let mut key = String::new();
        let mut closed = false;
        for next in chars.by_ref() {
            if next == '}' {
                closed = true;
                break;
            }
            key.push(next);
        }

        if !closed {
            return Err(ErrorKind::Template {
                operation: "render",
                message: "unclosed `{`".to_owned(),
            }
            .into());
        }

        if key.is_empty() {
            return Err(ErrorKind::Template {
                operation: "render",
                message: "empty key".to_owned(),
            }
            .into());
        }

        let value = values.get(&key).ok_or_else(|| ErrorKind::Missing {
            operation: "render",
            key: key.clone(),
        })?;
        output.push_str(value);
    }

    Ok(output)
}

fn title_word(word: &str) -> String {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };

    first
        .to_uppercase()
        .chain(chars.flat_map(char::to_lowercase))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    #[test]
    fn clean_collapses_whitespace() {
        assert_eq!(clean("  hello \n  world\t "), "hello world");
    }

    #[test]
    fn truncate_keeps_character_boundaries() {
        assert_eq!(truncate("你好世界", 2), "你好");
    }

    #[test]
    fn case_helpers_convert_text() {
        assert_eq!(lower("Ada"), "ada");
        assert_eq!(upper("Ada"), "ADA");
        assert_eq!(title("ada lovelace"), "Ada Lovelace");
    }

    #[test]
    fn slug_is_url_friendly() {
        assert_eq!(slug(" Hello, Rust World! "), "hello-rust-world");
    }

    #[test]
    fn render_replaces_template_values() -> std::result::Result<(), Box<dyn StdError>> {
        let output = render("hello {name}", [("name", "Ada")])?;

        assert_eq!(output, "hello Ada");
        Ok(())
    }

    #[test]
    fn render_missing_key_returns_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match render("hello {name}", [("other", "Ada")]) {
            Ok(output) => return Err(format!("expected missing key error, got {output}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Missing { operation, key } => {
                assert_eq!(*operation, "render");
                assert_eq!(key, "name");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }
}
