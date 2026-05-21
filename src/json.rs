//! 极简 JSON API。
//!
//! 这个模块使用直白的 Rust 命名：用 [`from_str`] / [`from_bytes`] 解析 JSON，用
//! [`to_string`] / [`to_string_pretty`] 序列化 JSON。它只负责 JSON 字符串、字节和 Rust 类型之间的转换，不处理文件；
//! JSON 文件请继续使用 `fs::read_json` 和 `fs::write_json`。

use std::{error::Error as StdError, fmt};

use serde::{Serialize, de::DeserializeOwned};

/// 动态 JSON 值类型。
///
/// 当 JSON 结构未知、字段不固定，或你想像 Python 的 `dict`/`list` 一样临时处理数据时，
/// 使用这个类型。结构明确时，优先使用 [`from_str`] 或 [`from_bytes`] 解析成自己的 Rust 结构体。
pub type Value = serde_json::Value;

/// 创建动态 JSON 值的宏。
///
/// 适合在测试、请求体或脚本式数据处理中直接写 JSON 字面量，例如
/// `json::value!({ "id": 1, "name": "Ada" })`。
pub use serde_json::json as value;

/// json 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。常见写法是 `let value = json::from_str(text)?;`。
pub type Result<T> = std::result::Result<T, Error>;

/// json 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分解析失败和序列化失败时，使用
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
    /// 调用方可以通过匹配 [`ErrorKind`] 区分 JSON 解析错误和 JSON 序列化错误。
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

/// json 模块的具体错误原因。
///
/// 每个错误都带有操作名，解析错误还会带输入预览，方便定位是哪段 JSON 失败。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// JSON 解析失败。
    #[error("json {operation} failed near `{input}`")]
    Decode {
        /// 发生错误的操作名，例如 `from_str`。
        operation: &'static str,
        /// 输入内容的短预览。
        input: String,
    },

    /// JSON 序列化失败。
    #[error("json {operation} failed")]
    Encode {
        /// 发生错误的操作名，例如 `to_string`。
        operation: &'static str,
    },
}

/// 把 JSON 文本解析成指定 Rust 类型。
///
/// 输入可以是 `&str` 或 `String`。类型 `T` 需要实现 serde 反序列化。解析失败会返回
/// [`ErrorKind::Decode`]，错误中包含 `from_str` 操作名和输入预览。
pub fn from_str<T>(text: impl AsRef<str>) -> Result<T>
where
    T: DeserializeOwned,
{
    let text = text.as_ref();
    serde_json::from_str(text).map_err(|source| {
        Error::with_source(
            ErrorKind::Decode {
                operation: "from_str",
                input: input_preview(text.as_bytes()),
            },
            source,
        )
    })
}

/// 把 JSON 字节解析成指定 Rust 类型。
///
/// 输入可以是 `&[u8]` 或 `Vec<u8>`。类型 `T` 需要实现 serde 反序列化。解析失败会返回
/// [`ErrorKind::Decode`]，错误中包含 `from_bytes` 操作名和输入预览。
pub fn from_bytes<T>(bytes: impl AsRef<[u8]>) -> Result<T>
where
    T: DeserializeOwned,
{
    let bytes = bytes.as_ref();
    serde_json::from_slice(bytes).map_err(|source| {
        Error::with_source(
            ErrorKind::Decode {
                operation: "from_bytes",
                input: input_preview(bytes),
            },
            source,
        )
    })
}

/// 把 JSON 文本解析成指定 Rust 类型，失败时返回默认值。
///
/// 这个函数仍然使用严格 JSON 规则，不支持注释、尾逗号或其它非标准写法。适合后台兜底展示，
/// 不适合需要发现配置错误的场景；需要错误信息时请使用 [`from_str`]。
pub fn from_str_or<T>(text: impl AsRef<str>, default: T) -> T
where
    T: DeserializeOwned,
{
    match from_str(text) {
        Ok(value) => value,
        Err(_) => default,
    }
}

/// 把 JSON 字节解析成指定 Rust 类型，失败时返回默认值。
///
/// 这个函数仍然使用严格 JSON 规则；需要错误信息时请使用 [`from_bytes`]。
pub fn from_bytes_or<T>(bytes: impl AsRef<[u8]>, default: T) -> T
where
    T: DeserializeOwned,
{
    match from_bytes(bytes) {
        Ok(value) => value,
        Err(_) => default,
    }
}

/// 把 JSON 文本解析成动态 JSON 值。
///
/// 适合处理结构未知或字段临时变化的数据。结构明确时，优先使用 [`from_str`] 解析成具体类型。
pub fn value_from_str(text: impl AsRef<str>) -> Result<Value> {
    let text = text.as_ref();
    serde_json::from_str(text).map_err(|source| {
        Error::with_source(
            ErrorKind::Decode {
                operation: "value_from_str",
                input: input_preview(text.as_bytes()),
            },
            source,
        )
    })
}

/// 把 JSON 字节解析成动态 JSON 值。
///
/// 适合处理结构未知或字段临时变化的字节输入。结构明确时，优先使用 [`from_bytes`] 解析成具体类型。
pub fn value_from_bytes(bytes: impl AsRef<[u8]>) -> Result<Value> {
    let bytes = bytes.as_ref();
    serde_json::from_slice(bytes).map_err(|source| {
        Error::with_source(
            ErrorKind::Decode {
                operation: "value_from_bytes",
                input: input_preview(bytes),
            },
            source,
        )
    })
}

/// 把 JSON 文本解析成动态 JSON 值，失败时返回默认值。
///
/// 这个函数仍然使用严格 JSON 规则；需要错误信息时请使用 [`value_from_str`]。
#[must_use]
pub fn value_from_str_or(text: impl AsRef<str>, default: Value) -> Value {
    match value_from_str(text) {
        Ok(value) => value,
        Err(_) => default,
    }
}

/// 把 JSON 字节解析成动态 JSON 值，失败时返回默认值。
///
/// 这个函数仍然使用严格 JSON 规则；需要错误信息时请使用 [`value_from_bytes`]。
#[must_use]
pub fn value_from_bytes_or(bytes: impl AsRef<[u8]>, default: Value) -> Value {
    match value_from_bytes(bytes) {
        Ok(value) => value,
        Err(_) => default,
    }
}

/// 把 JSON 文本解析成动态值，失败时返回 `null`。
///
/// 适合弱结构字段兜底展示。需要错误信息时请使用 [`value_from_str`]。
#[must_use]
pub fn value_or_null(text: impl AsRef<str>) -> Value {
    value_from_str_or(text, Value::Null)
}

/// 把 JSON 文本格式化成 pretty JSON，失败时返回原文本。
///
/// 适合后台展示 JSON 字段。这个函数不会接受非标准 JSON；解析失败时不修改原文本。
#[must_use]
pub fn pretty_or_original(text: impl AsRef<str>) -> String {
    let text = text.as_ref();
    match value_from_str(text).and_then(|value| to_string_pretty(&value)) {
        Ok(output) => output,
        Err(_) => text.to_owned(),
    }
}

/// 把 JSON 文本解析成 object，失败或根不是 object 时返回空 object。
///
/// 适合运行时扩展字段兜底。需要区分错误原因时请使用 [`value_from_str`] 后自行检查形状。
#[must_use]
pub fn object_or_empty(text: impl AsRef<str>) -> Value {
    match value_from_str(text) {
        Ok(Value::Object(object)) => Value::Object(object),
        _ => value!({}),
    }
}

/// 把 Rust 值序列化成紧凑 JSON 字符串。
///
/// 这个函数默认不加多余空格和换行，适合网络传输、存储和日志。需要人类可读输出时，使用
/// [`to_string_pretty`]。
pub fn to_string<T>(value: &T) -> Result<String>
where
    T: Serialize + ?Sized,
{
    serde_json::to_string(value).map_err(|source| {
        Error::with_source(
            ErrorKind::Encode {
                operation: "to_string",
            },
            source,
        )
    })
}

/// 把 Rust 值序列化成带缩进的 JSON 字符串。
///
/// 适合展示、配置模板和测试断言。需要紧凑输出时，使用 [`to_string`]。
pub fn to_string_pretty<T>(value: &T) -> Result<String>
where
    T: Serialize + ?Sized,
{
    serde_json::to_string_pretty(value).map_err(|source| {
        Error::with_source(
            ErrorKind::Encode {
                operation: "to_string_pretty",
            },
            source,
        )
    })
}

fn input_preview(input: &[u8]) -> String {
    const MAX_CHARS: usize = 160;

    let text = String::from_utf8_lossy(input);
    let mut preview = String::new();
    let mut truncated = false;

    for (index, character) in text.chars().enumerate() {
        if index == MAX_CHARS {
            truncated = true;
            break;
        }
        preview.push(character);
    }

    if truncated {
        preview.push_str("...");
    }

    preview
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize, ser};

    use super::*;

    #[derive(Debug, Deserialize, PartialEq, Serialize)]
    struct User {
        id: u64,
        name: String,
    }

    struct Broken;

    impl Serialize for Broken {
        fn serialize<S>(&self, _serializer: S) -> std::result::Result<S::Ok, S::Error>
        where
            S: ser::Serializer,
        {
            Err(ser::Error::custom("broken value"))
        }
    }

    #[test]
    fn from_str_parses_text_into_struct() -> std::result::Result<(), Box<dyn StdError>> {
        let user: User = from_str(r#"{"id":1,"name":"Ada"}"#)?;

        assert_eq!(
            user,
            User {
                id: 1,
                name: "Ada".to_owned(),
            }
        );
        Ok(())
    }

    #[test]
    fn from_bytes_parses_vec_bytes_into_struct() -> std::result::Result<(), Box<dyn StdError>> {
        let input = br#"{"id":2,"name":"Grace"}"#.to_vec();

        let user: User = from_bytes(input)?;

        assert_eq!(
            user,
            User {
                id: 2,
                name: "Grace".to_owned(),
            }
        );
        Ok(())
    }

    #[test]
    fn value_from_str_returns_dynamic_value() -> std::result::Result<(), Box<dyn StdError>> {
        let value = value_from_str(r#"{"id":1,"tags":["rust","json"]}"#)?;

        assert_eq!(value["id"], 1);
        assert_eq!(value["tags"][0], "rust");
        Ok(())
    }

    #[test]
    fn value_from_bytes_returns_dynamic_value() -> std::result::Result<(), Box<dyn StdError>> {
        let value = value_from_bytes(br#"{"id":1,"tags":["rust","json"]}"#)?;

        assert_eq!(value["id"], 1);
        assert_eq!(value["tags"][0], "rust");
        Ok(())
    }

    #[test]
    fn or_helpers_return_default_on_invalid_json() {
        let default = User {
            id: 0,
            name: "Default".to_owned(),
        };
        let parsed: User = from_str_or(r#"{"id":1,"name":"Ada"}"#, default);
        let fallback: User = from_bytes_or(
            b"{bad json",
            User {
                id: 2,
                name: "Fallback".to_owned(),
            },
        );
        let value = value_from_str_or("{bad json", value!({ "ok": false }));
        let bytes = value_from_bytes_or(br#"{"ok":true}"#, value!({ "ok": false }));

        assert_eq!(parsed.name, "Ada");
        assert_eq!(fallback.name, "Fallback");
        assert_eq!(value["ok"], false);
        assert_eq!(bytes["ok"], true);
    }

    #[test]
    fn display_or_helpers_keep_safe_fallbacks() {
        assert_eq!(value_or_null("{bad json"), Value::Null);
        assert_eq!(value_or_null(r#"{"ok":true}"#)["ok"], true);
        assert_eq!(pretty_or_original("{bad json"), "{bad json");
        assert!(pretty_or_original(r#"{"ok":true}"#).contains('\n'));
        assert_eq!(object_or_empty("{bad json"), value!({}));
        assert_eq!(object_or_empty(r#"[1,2]"#), value!({}));
        assert_eq!(object_or_empty(r#"{"ok":true}"#)["ok"], true);
    }

    #[test]
    fn value_macro_builds_dynamic_json() {
        let value = value!({
            "id": 1,
            "name": "Ada",
        });

        assert_eq!(value["id"], 1);
        assert_eq!(value["name"], "Ada");
    }

    #[test]
    fn to_string_outputs_compact_json() -> std::result::Result<(), Box<dyn StdError>> {
        let user = User {
            id: 1,
            name: "Ada".to_owned(),
        };

        let output = to_string(&user)?;

        assert_eq!(output, r#"{"id":1,"name":"Ada"}"#);
        Ok(())
    }

    #[test]
    fn to_string_pretty_outputs_indented_json() -> std::result::Result<(), Box<dyn StdError>> {
        let user = User {
            id: 1,
            name: "Ada".to_owned(),
        };

        let output = to_string_pretty(&user)?;

        assert!(output.contains('\n'));
        assert!(output.contains("  \"id\""));
        Ok(())
    }

    #[test]
    fn invalid_json_returns_decode_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match from_str::<User>("{bad json") {
            Ok(_) => return Err("expected JSON decode error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Decode {
                operation, input, ..
            } => {
                assert_eq!(*operation, "from_str");
                assert!(input.contains("{bad json"));
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[test]
    fn serialization_failure_returns_encode_error() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match to_string(&Broken) {
            Ok(_) => return Err("expected JSON encode error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Encode { operation, .. } => {
                assert_eq!(*operation, "to_string");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }
}
