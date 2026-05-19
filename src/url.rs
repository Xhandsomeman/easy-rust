//! 极简 URL API。
//!
//! 这个模块提供 URL 解析、拼接、查询串生成和组件编码解码。普通场景只需要
//! [`parse`]、[`join`]、[`encode`]、[`decode`] 和 [`query`]。

use std::{error::Error as StdError, fmt};

use serde::Serialize;

const INPUT_PREVIEW_CHARS: usize = 160;

/// url 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。常见写法是 `let api = url::parse(text)?;`。
pub type Result<T> = std::result::Result<T, Error>;

/// url 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分解析、拼接、解码或查询串错误时，
/// 使用 [`Error::kind`]。
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

/// url 模块的具体错误原因。
///
/// 错误信息会包含操作名和关键上下文，例如输入 URL、基础 URL、相对路径或输入预览。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// URL 解析失败。
    #[error("url {operation} `{input}` failed")]
    Parse {
        /// 发生错误的操作名，例如 `parse`。
        operation: &'static str,
        /// 输入内容预览。
        input: String,
    },

    /// URL 拼接失败。
    #[error("url {operation} base `{base}` path `{path}` failed")]
    Join {
        /// 发生错误的操作名，例如 `join`。
        operation: &'static str,
        /// 基础 URL。
        base: String,
        /// 待拼接的相对路径。
        path: String,
    },

    /// 百分号编码文本解码失败。
    #[error("url {operation} `{input}` failed: {message}")]
    Decode {
        /// 发生错误的操作名，例如 `decode`。
        operation: &'static str,
        /// 输入内容预览。
        input: String,
        /// 面向人的解码错误说明。
        message: String,
    },

    /// 查询串序列化失败。
    #[error("url {operation} failed")]
    Query {
        /// 发生错误的操作名，例如 `query`。
        operation: &'static str,
    },
}

/// 高层 URL 对象。
///
/// 这个类型只暴露常用读取方法和相对路径拼接，适合在应用代码里保存已经校验过的 URL。
#[derive(Clone, Eq, Hash, PartialEq)]
pub struct Url {
    inner: url_crate::Url,
}

impl Url {
    /// 返回完整 URL 字符串。
    #[must_use]
    pub fn as_str(&self) -> &str {
        self.inner.as_str()
    }

    /// 返回 URL 协议部分。
    ///
    /// 例如 `https://example.com` 的协议是 `https`。
    #[must_use]
    pub fn scheme(&self) -> &str {
        self.inner.scheme()
    }

    /// 返回主机名。
    ///
    /// URL 没有主机时返回 `None`。
    #[must_use]
    pub fn host(&self) -> Option<&str> {
        self.inner.host_str()
    }

    /// 返回路径部分。
    #[must_use]
    pub fn path(&self) -> &str {
        self.inner.path()
    }

    /// 返回查询串。
    ///
    /// 没有查询串时返回 `None`；返回值不包含开头的 `?`。
    #[must_use]
    pub fn query_string(&self) -> Option<&str> {
        self.inner.query()
    }

    /// 返回片段标识。
    ///
    /// 没有片段时返回 `None`；返回值不包含开头的 `#`。
    #[must_use]
    pub fn fragment(&self) -> Option<&str> {
        self.inner.fragment()
    }

    /// 返回端口号。
    ///
    /// URL 没有显式端口时返回 `None`。
    #[must_use]
    pub fn port(&self) -> Option<u16> {
        self.inner.port()
    }

    /// 基于当前 URL 拼接相对路径。
    ///
    /// 拼接失败会返回 [`ErrorKind::Join`]，错误包含基础 URL 和相对路径。
    pub fn join(&self, path: impl AsRef<str>) -> Result<Self> {
        let path = path.as_ref();
        self.inner
            .join(path)
            .map(|inner| Self { inner })
            .map_err(|source| {
                Error::with_source(
                    ErrorKind::Join {
                        operation: "join",
                        base: self.as_str().to_owned(),
                        path: path.to_owned(),
                    },
                    source,
                )
            })
    }
}

impl fmt::Display for Url {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.as_str())
    }
}

impl fmt::Debug for Url {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Url").field(&self.as_str()).finish()
    }
}

/// 解析 URL 字符串。
///
/// 解析成功返回高层 [`Url`]；输入非法时返回 [`ErrorKind::Parse`]，错误包含输入预览。
pub fn parse(input: impl AsRef<str>) -> Result<Url> {
    let input = input.as_ref();
    url_crate::Url::parse(input)
        .map(|inner| Url { inner })
        .map_err(|source| {
            Error::with_source(
                ErrorKind::Parse {
                    operation: "parse",
                    input: input_preview(input),
                },
                source,
            )
        })
}

/// 基于基础 URL 拼接相对路径。
///
/// `base` 可以是字符串或已解析的 URL 字符串。拼接失败时返回 [`ErrorKind::Join`]。
pub fn join(base: impl AsRef<str>, path: impl AsRef<str>) -> Result<Url> {
    let base = base.as_ref();
    let path = path.as_ref();
    let base_url = url_crate::Url::parse(base).map_err(|source| {
        Error::with_source(
            ErrorKind::Join {
                operation: "join",
                base: base.to_owned(),
                path: path.to_owned(),
            },
            source,
        )
    })?;

    base_url
        .join(path)
        .map(|inner| Url { inner })
        .map_err(|source| {
            Error::with_source(
                ErrorKind::Join {
                    operation: "join",
                    base: base.to_owned(),
                    path: path.to_owned(),
                },
                source,
            )
        })
}

/// 对 URL 组件做百分号编码。
///
/// 字母、数字和 `-._~` 会原样保留，其它字节会编码为 `%XX`。
#[must_use]
pub fn encode(text: impl AsRef<str>) -> String {
    let mut output = String::new();
    for byte in text.as_ref().bytes() {
        if is_unreserved(byte) {
            output.push(char::from(byte));
        } else {
            output.push('%');
            output.push(hex_char(byte >> 4));
            output.push(hex_char(byte & 0x0f));
        }
    }
    output
}

/// 解码百分号编码的 URL 组件。
///
/// 非法 `%` 转义或解码后不是 UTF-8 时返回 [`ErrorKind::Decode`]。这个函数只做百分号解码，
/// 不会把 `+` 转为空格。
pub fn decode(text: impl AsRef<str>) -> Result<String> {
    let text = text.as_ref();
    let bytes = text.as_bytes();
    let mut output = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        let byte = bytes[index];
        if byte != b'%' {
            output.push(byte);
            index += 1;
            continue;
        }

        if index + 2 >= bytes.len() {
            return Err(decode_error(text, "incomplete percent escape"));
        }

        let high = hex_value(bytes[index + 1]);
        let low = hex_value(bytes[index + 2]);
        match (high, low) {
            (Some(high), Some(low)) => output.push((high << 4) | low),
            _ => return Err(decode_error(text, "invalid percent escape")),
        }
        index += 3;
    }

    String::from_utf8(output).map_err(|_| decode_error(text, "decoded bytes are not utf-8"))
}

/// 把键值结构序列化为 URL 查询串。
///
/// 常见输入是结构体、数组或 `Vec<(key, value)>`。返回值不包含开头的 `?`；空格会按
/// 查询串语义编码为 `+`。
pub fn query<T>(params: &T) -> Result<String>
where
    T: Serialize + ?Sized,
{
    serde_urlencoded::to_string(params)
        .map_err(|source| Error::with_source(ErrorKind::Query { operation: "query" }, source))
}

fn decode_error(input: &str, message: &str) -> Error {
    ErrorKind::Decode {
        operation: "decode",
        input: input_preview(input),
        message: message.to_owned(),
    }
    .into()
}

fn input_preview(input: &str) -> String {
    let mut output = String::new();
    let mut chars = input.chars();

    for _ in 0..INPUT_PREVIEW_CHARS {
        let Some(ch) = chars.next() else {
            return output;
        };
        output.push(ch);
    }

    if chars.next().is_some() {
        output.push_str("...");
    }

    output
}

fn is_unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~')
}

fn hex_char(value: u8) -> char {
    char::from(match value {
        0..=9 => b'0' + value,
        _ => b'A' + (value - 10),
    })
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use serde::Serialize;

    use super::*;

    #[derive(Serialize)]
    struct Search<'a> {
        q: &'a str,
        page: u8,
    }

    #[test]
    fn parse_reads_url_parts() -> std::result::Result<(), Box<dyn StdError>> {
        let url = parse("https://example.com:8443/api/users?q=rust#top")?;

        assert_eq!(
            url.as_str(),
            "https://example.com:8443/api/users?q=rust#top"
        );
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host(), Some("example.com"));
        assert_eq!(url.port(), Some(8443));
        assert_eq!(url.path(), "/api/users");
        assert_eq!(url.query_string(), Some("q=rust"));
        assert_eq!(url.fragment(), Some("top"));
        Ok(())
    }

    #[test]
    fn join_combines_base_and_path() -> std::result::Result<(), Box<dyn StdError>> {
        let url = join("https://example.com/api/", "users/1")?;
        let next = url.join("../health")?;

        assert_eq!(url.as_str(), "https://example.com/api/users/1");
        assert_eq!(next.as_str(), "https://example.com/api/health");
        Ok(())
    }

    #[test]
    fn join_with_invalid_base_returns_join_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match join("not a url", "users/1") {
            Ok(value) => return Err(format!("expected join error, got {value}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Join {
                operation,
                base,
                path,
            } => {
                assert_eq!(*operation, "join");
                assert_eq!(base, "not a url");
                assert_eq!(path, "users/1");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        let message = error.to_string();
        assert!(message.contains("join"));
        assert!(message.contains("not a url"));
        assert!(message.contains("users/1"));
        Ok(())
    }

    #[test]
    fn debug_is_high_level() -> std::result::Result<(), Box<dyn StdError>> {
        let url = parse("https://example.com/api")?;

        assert_eq!(format!("{url:?}"), r#"Url("https://example.com/api")"#);
        Ok(())
    }

    #[test]
    fn encode_and_decode_components() -> std::result::Result<(), Box<dyn StdError>> {
        let encoded = encode("Ada Lovelace/你好");
        let decoded = decode(&encoded)?;

        assert_eq!(encoded, "Ada%20Lovelace%2F%E4%BD%A0%E5%A5%BD");
        assert_eq!(decoded, "Ada Lovelace/你好");
        Ok(())
    }

    #[test]
    fn decode_keeps_plus_literal() -> std::result::Result<(), Box<dyn StdError>> {
        assert_eq!(decode("rust+url")?, "rust+url");
        Ok(())
    }

    #[test]
    fn query_serializes_structs() -> std::result::Result<(), Box<dyn StdError>> {
        let output = query(&Search {
            q: "rust url",
            page: 2,
        })?;

        assert_eq!(output, "q=rust+url&page=2");
        Ok(())
    }

    #[test]
    fn invalid_inputs_return_context_errors() -> std::result::Result<(), Box<dyn StdError>> {
        let parse_error = match parse("not a url") {
            Ok(value) => return Err(format!("expected parse error, got {value}").into()),
            Err(error) => error,
        };
        let decode_error = match decode("%zz") {
            Ok(value) => return Err(format!("expected decode error, got {value}").into()),
            Err(error) => error,
        };

        assert!(parse_error.to_string().contains("parse"));
        assert!(parse_error.to_string().contains("not a url"));
        assert!(decode_error.to_string().contains("decode"));
        assert!(decode_error.to_string().contains("%zz"));
        Ok(())
    }
}
