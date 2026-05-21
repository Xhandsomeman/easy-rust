//! 极简字符串处理 API。
//!
//! 这个模块提供脚本和后端最常用的字符串处理能力：清洗空白、按标记截取、截断、大小写转换、slug 和简单模板替换。

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

/// 把 HTML 转成可读文本。
///
/// 会去掉标签、解码常见 HTML 特殊字符写法，并使用 [`clean`] 清理连续空白。适合从网页片段中
/// 提取简单正文，不做 JavaScript 渲染或复杂排版还原。
#[must_use]
pub fn html_to_text(html: impl AsRef<str>) -> String {
    let fragment = scraper_crate::Html::parse_fragment(html.as_ref());
    clean(html_decode(
        fragment.root_element().text().collect::<Vec<_>>().join(" "),
    ))
}

/// 解码 HTML 特殊字符写法。
///
/// 适合处理 API、RSS、meta 字段或数据库文本中的 `&amp;`、`&lt;`、`&#19990;`、`&#x4e16;`。
/// 未知或非法写法会原样保留，避免破坏原始文本。
#[must_use]
pub fn html_decode(text: impl AsRef<str>) -> String {
    let mut output = String::with_capacity(text.as_ref().len());
    let mut rest = text.as_ref();

    while let Some(start) = rest.find('&') {
        output.push_str(&rest[..start]);
        let after_amp = &rest[start + 1..];

        let Some(end) = after_amp.find(';') else {
            output.push_str(&rest[start..]);
            return output;
        };

        let name = &after_amp[..end];
        if let Some(decoded) = decode_html_name(name) {
            output.push_str(&decoded);
        } else {
            output.push('&');
            output.push_str(name);
            output.push(';');
        }

        rest = &after_amp[end + 1..];
    }

    output.push_str(rest);
    output
}

/// 把全角 ASCII 转成半角 ASCII。
///
/// 会把全角空格转成普通空格，把全角字母、数字和常见 ASCII 标点转成半角。不会处理假名、繁简
/// 或更广泛的 Unicode 归一化。
#[must_use]
pub fn to_half_width(text: impl AsRef<str>) -> String {
    text.as_ref()
        .chars()
        .map(|character| match character {
            '\u{3000}' => ' ',
            '\u{ff01}'..='\u{ff5e}' => {
                char::from_u32(u32::from(character) - 0xfee0).unwrap_or(character)
            }
            _ => character,
        })
        .collect()
}

/// 判断文本是否为空或只包含空白。
///
/// 适合校验表单、配置值和采集字段。会使用 Rust 的 Unicode 空白规则；空字符串和只包含空格、
/// 换行、制表符的字符串都会返回 `true`。
#[must_use]
pub fn is_blank(text: impl AsRef<str>) -> bool {
    text.as_ref().trim().is_empty()
}

/// 文本为空白时返回默认值。
///
/// 适合后台表单、配置展示和日志字段兜底。`text` 为空字符串或只包含空白时返回 `default`；
/// 否则返回原文本。
#[must_use]
pub fn blank_or(text: impl AsRef<str>, default: impl ToString) -> String {
    let text = text.as_ref();
    if is_blank(text) {
        default.to_string()
    } else {
        text.to_owned()
    }
}

/// 把字节数格式化成易读文本。
///
/// 使用 1024 进制单位，常见输出如 `512 B`、`1 KB`、`1.5 KB`、`2 MB`。适合日志、
/// 后台列表和错误信息展示。
#[must_use]
pub fn format_bytes(size: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB", "TB", "PB"];

    if size < 1024 {
        return format!("{size} B");
    }

    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }

    if value.fract() == 0.0 || value >= 10.0 {
        format!("{value:.0} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// 把以分为单位的金额格式化成普通数字文本。
///
/// 不添加币种符号，只输出千分位和两位小数，例如 `123456` 会变成 `1,234.56`。
/// 负数会保留 `-`。
#[must_use]
pub fn format_money(cents: i64) -> String {
    let negative = cents.is_negative();
    let value = cents.unsigned_abs();
    let whole = value / 100;
    let fraction = value % 100;
    let mut digits = whole.to_string();
    let mut grouped = String::new();

    while digits.len() > 3 {
        let chunk = digits.split_off(digits.len() - 3);
        if grouped.is_empty() {
            grouped = chunk;
        } else {
            grouped = format!("{chunk},{grouped}");
        }
    }

    if grouped.is_empty() {
        grouped = digits;
    } else {
        grouped = format!("{digits},{grouped}");
    }

    if negative {
        format!("-{grouped}.{fraction:02}")
    } else {
        format!("{grouped}.{fraction:02}")
    }
}

/// 提取两个标记之间的第一段文本。
///
/// 会先找到 `start`，再从它后面寻找 `end`，返回中间内容。任一标记为空、找不到开始标记或结束
/// 标记时返回 `None`。适合从固定格式文本中取出一小段内容。
#[must_use]
pub fn between(
    text: impl AsRef<str>,
    start: impl AsRef<str>,
    end: impl AsRef<str>,
) -> Option<String> {
    let text = text.as_ref();
    let start = start.as_ref();
    let end = end.as_ref();

    if start.is_empty() || end.is_empty() {
        return None;
    }

    let (_, rest) = text.split_once(start)?;
    let (middle, _) = rest.split_once(end)?;
    Some(middle.to_owned())
}

/// 提取第一个标记之前的文本。
///
/// 找到 `marker` 时返回它左侧的内容；标记为空或不存在时返回 `None`。适合读取 `key=value`
/// 这类简单文本的前半部分。
#[must_use]
pub fn before(text: impl AsRef<str>, marker: impl AsRef<str>) -> Option<String> {
    let marker = marker.as_ref();
    if marker.is_empty() {
        return None;
    }

    text.as_ref()
        .split_once(marker)
        .map(|(left, _)| left.to_owned())
}

/// 提取第一个标记之后的文本。
///
/// 找到 `marker` 时返回它右侧的内容；标记为空或不存在时返回 `None`。适合读取 `key=value`
/// 这类简单文本的后半部分。
#[must_use]
pub fn after(text: impl AsRef<str>, marker: impl AsRef<str>) -> Option<String> {
    let marker = marker.as_ref();
    if marker.is_empty() {
        return None;
    }

    text.as_ref()
        .split_once(marker)
        .map(|(_, right)| right.to_owned())
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

fn decode_html_name(name: &str) -> Option<String> {
    let character = match name {
        "amp" => return Some("&".to_owned()),
        "lt" => return Some("<".to_owned()),
        "gt" => return Some(">".to_owned()),
        "quot" => return Some("\"".to_owned()),
        "apos" => return Some("'".to_owned()),
        "nbsp" => return Some(" ".to_owned()),
        "ldquo" => return Some("\u{201c}".to_owned()),
        "rdquo" => return Some("\u{201d}".to_owned()),
        "lsquo" => return Some("\u{2018}".to_owned()),
        "rsquo" => return Some("\u{2019}".to_owned()),
        "hellip" => return Some("\u{2026}".to_owned()),
        "mdash" => return Some("\u{2014}".to_owned()),
        "ndash" => return Some("\u{2013}".to_owned()),
        decimal if decimal.starts_with('#') => decode_html_number(decimal)?,
        _ => return None,
    };
    Some(character.to_string())
}

fn decode_html_number(name: &str) -> Option<char> {
    let number = &name[1..];
    let value = number
        .strip_prefix('x')
        .or_else(|| number.strip_prefix('X'))
        .map_or_else(
            || number.parse::<u32>().ok(),
            |hex| u32::from_str_radix(hex, 16).ok(),
        )?;
    char::from_u32(value)
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
    fn html_to_text_removes_tags_and_decodes_html_text() {
        let html =
            r#"<main><h1>Ada&nbsp;&amp;&nbsp;Grace</h1><p>Hello <b>Rust</b> &lt;3</p></main>"#;

        assert_eq!(html_to_text(html), "Ada & Grace Hello Rust <3");
    }

    #[test]
    fn html_decode_decodes_known_and_numeric_html_text() {
        let text = "&ldquo;标题&rdquo;&hellip;&#x4e16;&#30028;&amp;nbsp;";

        assert_eq!(html_decode(text), "\u{201c}标题\u{201d}\u{2026}世界&nbsp;");
        assert_eq!(
            html_decode("&unknown; &#xzz; &#99999999; &amp"),
            "&unknown; &#xzz; &#99999999; &amp"
        );
    }

    #[test]
    fn to_half_width_converts_full_width_ascii() {
        assert_eq!(to_half_width("ＡＢＣ１２３　test！＠＃"), "ABC123 test!@#");
        assert_eq!(to_half_width("カタカナ"), "カタカナ");
    }

    #[test]
    fn is_blank_checks_empty_or_whitespace_text() {
        assert!(is_blank(""));
        assert!(is_blank(" \n\t "));
        assert!(!is_blank(" rust "));
    }

    #[test]
    fn blank_or_and_format_helpers_make_display_text() {
        assert_eq!(blank_or("", "N/A"), "N/A");
        assert_eq!(blank_or(" \n ", "N/A"), "N/A");
        assert_eq!(blank_or("Ada", "N/A"), "Ada");
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(1024), "1 KB");
        assert_eq!(format_bytes(1536), "1.5 KB");
        assert_eq!(format_bytes(2 * 1024 * 1024), "2 MB");
        assert_eq!(format_money(0), "0.00");
        assert_eq!(format_money(123_456), "1,234.56");
        assert_eq!(format_money(-123_456), "-1,234.56");
    }

    #[test]
    fn between_extracts_text_between_markers() {
        assert_eq!(
            between("我喜欢美国的拉斯维加斯", "喜欢", "的"),
            Some("美国".to_owned())
        );
        assert_eq!(
            between("a[start]first[end] second[end]", "[start]", "[end]"),
            Some("first".to_owned())
        );
        assert_eq!(between("hello", "[", "]"), None);
        assert_eq!(between("hello", "", "]"), None);
        assert_eq!(between("hello", "[", ""), None);
    }

    #[test]
    fn before_and_after_extract_text_around_marker() {
        assert_eq!(before("name=Ada=Lovelace", "="), Some("name".to_owned()));
        assert_eq!(
            after("name=Ada=Lovelace", "="),
            Some("Ada=Lovelace".to_owned())
        );
        assert_eq!(before("name", "="), None);
        assert_eq!(after("name", "="), None);
        assert_eq!(before("name", ""), None);
        assert_eq!(after("name", ""), None);
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
