//! 极简数据集合 API。
//!
//! 这个模块把常见列表、字典、集合、分组、计数和分块操作做成立即返回结果的高层函数。
//! 它不要求用户导入额外能力，也不会返回需要继续组合的惰性对象。

use std::{
    any::type_name,
    borrow::Borrow,
    collections::{HashMap, HashSet, hash_map::Entry},
    error::Error as StdError,
    fmt,
    hash::Hash,
    str::FromStr,
};

use crate::json::{self, Value};

/// data 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。当前主要用于 [`chunked`] 处理无效分块大小。
pub type Result<T> = std::result::Result<T, Error>;

/// data 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分数据处理错误时，使用 [`Error::kind`]。
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

/// data 模块的具体错误原因。
///
/// 错误信息会包含操作名和无效参数，方便定位是哪一次数据处理调用失败。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 分块大小不合法。
    #[error("data {operation} failed: size must be greater than 0, got {size}")]
    InvalidSize {
        /// 发生错误的操作名，例如 `chunked`。
        operation: &'static str,
        /// 调用方传入的无效大小。
        size: usize,
    },

    /// 必填表单字段不存在。
    #[error("data {operation} form `{key}` is required")]
    Required {
        /// 发生错误的操作名，例如 `require`。
        operation: &'static str,
        /// 缺失的字段名。
        key: String,
    },

    /// 表单字段类型转换失败。
    #[error("data {operation} form `{key}` value `{value}` failed: expected {expected}: {message}")]
    Type {
        /// 发生错误的操作名，例如 `get`。
        operation: &'static str,
        /// 发生错误的字段名。
        key: String,
        /// 发生错误的字段值。
        value: String,
        /// 期望的 Rust 类型名。
        expected: &'static str,
        /// 面向人的错误说明。
        message: String,
    },
}

/// Python `Counter` 风格的计数结果。
///
/// 使用 [`counter`] 创建。它会记录每个元素出现次数、总元素数量和首次出现顺序；这让
/// [`Counter::most_common`] 在次数相同时可以保持稳定顺序。
#[derive(Clone, Debug)]
pub struct Counter<T> {
    counts: HashMap<T, usize>,
    order: Vec<T>,
    total: usize,
}

impl<T> Counter<T>
where
    T: Eq + Hash,
{
    /// 返回某个元素出现的次数。
    ///
    /// 元素不存在时返回 `0`，对齐 Python `Counter` 的常用体验。
    #[must_use]
    pub fn get<Q>(&self, item: &Q) -> usize
    where
        T: Borrow<Q>,
        Q: Eq + Hash + ?Sized,
    {
        self.counts.get(item).copied().unwrap_or(0)
    }

    /// 返回所有元素出现次数的总和。
    ///
    /// 这个值等于创建 [`Counter`] 时输入迭代器的元素总数。
    #[must_use]
    pub fn total(&self) -> usize {
        self.total
    }

    /// 返回不同元素的数量。
    ///
    /// 这个值等于计数表中 key 的数量，不是所有元素出现次数的总和。
    #[must_use]
    pub fn len(&self) -> usize {
        self.counts.len()
    }

    /// 判断计数结果是否为空。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }
}

/// 运行时表单字段集合。
///
/// 使用 [`form`] 创建。适合处理动态表单、查询参数或后台字段列表；字段值统一先按文本保存，
/// 读取时再转换成需要的类型。空字符串和纯空白字段按缺失处理。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Form {
    values: HashMap<String, Vec<String>>,
}

impl Form {
    /// 读取可选字段并转换类型。
    ///
    /// 字段不存在或字段值为空白时返回 `Ok(None)`；字段存在但不能转换成 `T` 时返回
    /// [`ErrorKind::Type`]。
    pub fn get<T>(&self, name: impl AsRef<str>) -> Result<Option<T>>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        self.parse_value("get", name.as_ref())
    }

    /// 读取字段，缺失时返回默认值。
    ///
    /// 字段不存在或字段值为空白时返回 `default`；字段存在但不能转换成 `T` 时返回
    /// [`ErrorKind::Type`]。
    pub fn get_or<T>(&self, name: impl AsRef<str>, default: T) -> Result<T>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        Ok(self
            .parse_value("get_or", name.as_ref())?
            .unwrap_or(default))
    }

    /// 读取必填字段并转换类型。
    ///
    /// 字段不存在或字段值为空白时返回 [`ErrorKind::Required`]；字段存在但不能转换成 `T` 时返回
    /// [`ErrorKind::Type`]。
    pub fn require<T>(&self, name: impl AsRef<str>) -> Result<T>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        let name = name.as_ref();
        self.parse_value("require", name)?.ok_or_else(|| {
            ErrorKind::Required {
                operation: "require",
                key: name.to_owned(),
            }
            .into()
        })
    }

    /// 读取字段文本。
    ///
    /// 重复字段会返回最后一个非空白值；字段不存在或全部为空白时返回 `None`。
    #[must_use]
    pub fn text(&self, name: impl AsRef<str>) -> Option<String> {
        self.last_value(name.as_ref()).map(str::to_owned)
    }

    /// 返回字段出现过的所有非空白值。
    ///
    /// 返回顺序保持输入顺序。空字符串和纯空白字段会被跳过。
    #[must_use]
    pub fn values(&self, name: impl AsRef<str>) -> Vec<String> {
        self.values
            .get(name.as_ref())
            .into_iter()
            .flat_map(|values| values.iter())
            .filter(|value| !value.trim().is_empty())
            .cloned()
            .collect()
    }

    /// 把字段读取成文本列表。
    ///
    /// 重复字段会全部参与拆分；单个字段里可以用逗号、分号、竖线或空白分隔。
    #[must_use]
    pub fn list(&self, name: impl AsRef<str>) -> Vec<String> {
        self.values(name).into_iter().flat_map(parse_list).collect()
    }

    /// 把字段读取成 JSON 动态值。
    ///
    /// 字段缺失、空白或 JSON 不合法时返回 `None`。
    #[must_use]
    pub fn json(&self, name: impl AsRef<str>) -> Option<Value> {
        self.text(name).and_then(parse_json)
    }

    /// 把字段读取成 JSON object 或 array。
    ///
    /// 字段缺失、空白、JSON 不合法或根不是 object/array 时返回 `None`。
    #[must_use]
    pub fn json_collection(&self, name: impl AsRef<str>) -> Option<Value> {
        self.text(name).and_then(parse_json_collection)
    }

    /// 把字段读取成键值条目列表。
    ///
    /// 字段值支持 `key=value` 或 `key: value`，多项可用换行、逗号或分号分隔。
    #[must_use]
    pub fn key_values(&self, name: impl AsRef<str>) -> Vec<KeyValue> {
        self.text(name).map_or_else(Vec::new, parse_key_values)
    }

    fn parse_value<T>(&self, operation: &'static str, name: &str) -> Result<Option<T>>
    where
        T: FromStr,
        T::Err: fmt::Display,
    {
        let Some(value) = self.last_value(name) else {
            return Ok(None);
        };
        value.parse::<T>().map(Some).map_err(|source| {
            ErrorKind::Type {
                operation,
                key: name.to_owned(),
                value: value.to_owned(),
                expected: type_name::<T>(),
                message: source.to_string(),
            }
            .into()
        })
    }

    fn last_value(&self, name: &str) -> Option<&str> {
        self.values
            .get(name)?
            .iter()
            .rev()
            .find(|value| !value.trim().is_empty())
            .map(String::as_str)
    }
}

/// 简单键值条目。
///
/// 使用 [`parse_key_values`] 创建，适合运行时表单、配置中心或导入字段中的 `key=value` 文本。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KeyValue {
    /// 键名。
    pub key: String,
    /// 值文本。
    pub value: String,
}

impl<T> Counter<T>
where
    T: Clone + Eq + Hash,
{
    /// 按出现次数从高到低返回计数结果。
    ///
    /// 次数相同时，保持元素第一次出现的顺序。返回值会克隆 key，避免暴露内部存储结构。
    #[must_use]
    pub fn most_common(&self) -> Vec<(T, usize)> {
        let mut items: Vec<(usize, T, usize)> = self
            .order
            .iter()
            .enumerate()
            .map(|(index, item)| (index, item.clone(), self.get(item)))
            .collect();

        items.sort_by(|left, right| right.2.cmp(&left.2).then_with(|| left.0.cmp(&right.0)));

        items
            .into_iter()
            .map(|(_, item, count)| (item, count))
            .collect()
    }
}

/// 把任意迭代器收集成列表。
///
/// 这是 `iter.into_iter().collect::<Vec<_>>()` 的短入口，适合普通脚本式数据处理。
#[must_use]
pub fn list<I>(iter: I) -> Vec<I::Item>
where
    I: IntoIterator,
{
    iter.into_iter().collect()
}

/// 把元素连接成字符串。
///
/// 这是 `items.join(sep)` 的函数式入口，适合把数字、字符串或其它可转成文本的元素拼成一行。
/// 空列表会返回空字符串。
#[must_use]
pub fn join<I>(items: I, sep: impl AsRef<str>) -> String
where
    I: IntoIterator,
    I::Item: ToString,
{
    let sep = sep.as_ref();
    let mut output = String::new();
    let mut first = true;

    for item in items {
        if first {
            first = false;
        } else {
            output.push_str(sep);
        }
        output.push_str(&item.to_string());
    }

    output
}

/// 把任意迭代器收集成集合。
///
/// 使用标准库 [`HashSet`]；不承诺迭代顺序。重复元素会自动去重。
#[must_use]
pub fn set<I>(iter: I) -> HashSet<I::Item>
where
    I: IntoIterator,
    I::Item: Eq + Hash,
{
    iter.into_iter().collect()
}

/// 把 key-value pair 迭代器收集成字典。
///
/// 使用标准库 [`HashMap`]；不承诺迭代顺序。重复 key 会以后出现的值为准。
#[must_use]
pub fn dict<I, K, V>(pairs: I) -> HashMap<K, V>
where
    I: IntoIterator<Item = (K, V)>,
    K: Eq + Hash,
{
    pairs.into_iter().collect()
}

/// 把键值列表收集成运行时表单。
///
/// 重复字段会全部保留；读取单个值时使用最后一个非空白值。适合把 HTTP 表单、查询参数或
/// 临时字段列表转换成可按类型读取的结构。
#[must_use]
pub fn form<I, K, V>(items: I) -> Form
where
    I: IntoIterator<Item = (K, V)>,
    K: AsRef<str>,
    V: ToString,
{
    let mut form = Form::default();
    for (key, value) in items {
        form.values
            .entry(key.as_ref().to_owned())
            .or_default()
            .push(value.to_string());
    }
    form
}

/// 把文本解析成布尔值。
///
/// 支持 `true/false`、`1/0`、`yes/no`、`on/off`，大小写不敏感；空白或未知值返回 `None`。
#[must_use]
pub fn parse_bool(text: impl AsRef<str>) -> Option<bool> {
    match text.as_ref().trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// 把文本解析成数字或其它实现 [`FromStr`] 的类型。
///
/// 空白或转换失败时返回 `None`。
#[must_use]
pub fn parse_number<T>(text: impl AsRef<str>) -> Option<T>
where
    T: FromStr,
{
    text.as_ref().trim().parse().ok()
}

/// 把文本解析成可选字符串。
///
/// 空字符串或纯空白返回 `None`；其它文本返回去掉首尾空白后的字符串。
#[must_use]
pub fn parse_text(text: impl AsRef<str>) -> Option<String> {
    let text = text.as_ref().trim();
    if text.is_empty() {
        None
    } else {
        Some(text.to_owned())
    }
}

/// 把文本解析成字符串列表。
///
/// 会按逗号、分号、竖线和空白拆分，自动去掉空项，不做去重。
#[must_use]
pub fn parse_list(text: impl AsRef<str>) -> Vec<String> {
    text.as_ref()
        .split(|character: char| character.is_whitespace() || matches!(character, ',' | ';' | '|'))
        .filter_map(parse_text)
        .collect()
}

/// 把文本解析成 JSON 动态值。
///
/// 空白或 JSON 不合法时返回 `None`。
#[must_use]
pub fn parse_json(text: impl AsRef<str>) -> Option<Value> {
    parse_text(text).and_then(|text| json::value_from_str(text).ok())
}

/// 把文本解析成 JSON object 或 array。
///
/// 根不是 object 或 array 时返回 `None`。
#[must_use]
pub fn parse_json_collection(text: impl AsRef<str>) -> Option<Value> {
    match parse_json(text)? {
        value @ (Value::Array(_) | Value::Object(_)) => Some(value),
        _ => None,
    }
}

/// 把文本解析成键值条目列表。
///
/// 支持 `key=value` 和 `key: value`；多项可用换行、逗号或分号分隔。空 key 会被跳过。
#[must_use]
pub fn parse_key_values(text: impl AsRef<str>) -> Vec<KeyValue> {
    text.as_ref()
        .split(['\n', '\r', ',', ';'])
        .filter_map(|entry| {
            let entry = entry.trim();
            let (key, value) = entry.split_once('=').or_else(|| entry.split_once(':'))?;
            let key = key.trim();
            if key.is_empty() {
                return None;
            }
            Some(KeyValue {
                key: key.to_owned(),
                value: value.trim().to_owned(),
            })
        })
        .collect()
}

/// 对每个元素执行映射，并立即返回列表。
///
/// 闭包接收 owned item，适合把输入直接转换成新的列表。
#[must_use]
pub fn map<I, F, U>(iter: I, f: F) -> Vec<U>
where
    I: IntoIterator,
    F: FnMut(I::Item) -> U,
{
    iter.into_iter().map(f).collect()
}

/// 过滤元素，并立即返回列表。
///
/// 闭包接收元素引用，避免为了判断条件而隐藏 clone。通过条件的元素会按原顺序返回。
#[must_use]
pub fn filter<I, F>(iter: I, mut f: F) -> Vec<I::Item>
where
    I: IntoIterator,
    F: FnMut(&I::Item) -> bool,
{
    iter.into_iter().filter(|item| f(item)).collect()
}

/// 统计每个元素出现次数。
///
/// 返回的 [`Counter`] 支持查询单个元素次数、总数和最常见元素列表。
#[must_use]
pub fn counter<I>(iter: I) -> Counter<I::Item>
where
    I: IntoIterator,
    I::Item: Clone + Eq + Hash,
{
    let mut counts = HashMap::new();
    let mut order = Vec::new();
    let mut total = 0;

    for item in iter {
        match counts.entry(item) {
            Entry::Occupied(mut entry) => {
                *entry.get_mut() += 1;
            }
            Entry::Vacant(entry) => {
                order.push(entry.key().clone());
                entry.insert(1);
            }
        }
        total += 1;
    }

    Counter {
        counts,
        order,
        total,
    }
}

/// 按 key 分组。
///
/// 闭包接收元素引用并返回分组 key。每个分组内部会保留输入顺序；返回的 [`HashMap`] 不承诺
/// 分组 key 的迭代顺序。
#[must_use]
pub fn group_by<I, F, K>(iter: I, mut key_fn: F) -> HashMap<K, Vec<I::Item>>
where
    I: IntoIterator,
    F: FnMut(&I::Item) -> K,
    K: Eq + Hash,
{
    let mut groups: HashMap<K, Vec<I::Item>> = HashMap::new();

    for item in iter {
        let key = key_fn(&item);
        groups.entry(key).or_default().push(item);
    }

    groups
}

/// 把输入按固定大小分块。
///
/// 最后一块不足 `size` 时会保留尾块。`size == 0` 会返回 [`ErrorKind::InvalidSize`]，
/// 避免 panic 或静默吞掉调用错误。
pub fn chunked<I>(iter: I, size: usize) -> Result<Vec<Vec<I::Item>>>
where
    I: IntoIterator,
{
    if size == 0 {
        return Err(ErrorKind::InvalidSize {
            operation: "chunked",
            size,
        }
        .into());
    }

    let mut chunks = Vec::new();
    let mut current = Vec::with_capacity(size);

    for item in iter {
        current.push(item);

        if current.len() == size {
            chunks.push(current);
            current = Vec::with_capacity(size);
        }
    }

    if !current.is_empty() {
        chunks.push(current);
    }

    Ok(chunks)
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    #[derive(Clone, Debug, Eq, Hash, PartialEq)]
    struct User {
        name: &'static str,
        role: &'static str,
    }

    #[test]
    fn list_set_and_dict_build_collections() {
        let nums = list([1, 2, 3]);
        let names = set(["Ada", "Grace", "Ada"]);
        let ages = dict([("Ada", 36), ("Grace", 85), ("Ada", 37)]);

        assert_eq!(nums, vec![1, 2, 3]);
        assert_eq!(names.len(), 2);
        assert_eq!(ages.get("Ada"), Some(&37));
        assert_eq!(ages.get("Grace"), Some(&85));
    }

    #[test]
    fn map_and_filter_return_ordered_vecs() {
        let doubled = map([1, 2, 3], |item| item * 2);
        let evens = filter([1, 2, 3, 4], |item| *item % 2 == 0);

        assert_eq!(doubled, vec![2, 4, 6]);
        assert_eq!(evens, vec![2, 4]);
    }

    #[test]
    fn join_turns_items_into_text() {
        assert_eq!(join(["a", "b", "c"], ","), "a,b,c");
        assert_eq!(join([1, 2, 3], "-"), "1-2-3");
        assert_eq!(join(["only"], ","), "only");
        assert_eq!(join(Vec::<String>::new(), ","), "");
    }

    #[test]
    fn form_reads_runtime_values_with_types_and_defaults()
    -> std::result::Result<(), Box<dyn StdError>> {
        let form = form([
            ("name", "Ada"),
            ("page", "1"),
            ("debug", "true"),
            ("tag", "rust"),
            ("tag", "easy"),
            ("blank", "  "),
            ("items", "a,b c|d"),
            ("meta", r#"{"ok":true}"#),
            ("entries", "a=1\nb: 2"),
        ]);

        assert_eq!(form.text("name"), Some("Ada".to_owned()));
        assert_eq!(form.get::<u32>("page")?, Some(1));
        assert!(form.require::<bool>("debug")?);
        assert_eq!(form.get_or("limit", 20_u32)?, 20);
        assert_eq!(form.get::<String>("blank")?, None);
        assert_eq!(
            form.values("tag"),
            vec!["rust".to_owned(), "easy".to_owned()]
        );
        assert_eq!(form.list("items"), vec!["a", "b", "c", "d"]);
        assert_eq!(
            form.json("meta")
                .and_then(|value| value.get("ok").and_then(Value::as_bool)),
            Some(true)
        );
        assert_eq!(
            form.json_collection("meta")
                .and_then(|value| value.get("ok").and_then(Value::as_bool)),
            Some(true)
        );
        assert_eq!(
            form.key_values("entries"),
            vec![
                KeyValue {
                    key: "a".to_owned(),
                    value: "1".to_owned(),
                },
                KeyValue {
                    key: "b".to_owned(),
                    value: "2".to_owned(),
                },
            ]
        );
        Ok(())
    }

    #[test]
    fn parse_helpers_cover_runtime_field_values() {
        assert_eq!(parse_bool("YES"), Some(true));
        assert_eq!(parse_bool("off"), Some(false));
        assert_eq!(parse_bool("maybe"), None);
        assert_eq!(parse_number::<u32>("42"), Some(42));
        assert_eq!(parse_number::<u32>("bad"), None);
        assert_eq!(parse_text("  Ada  "), Some("Ada".to_owned()));
        assert_eq!(parse_text(" \n "), None);
        assert_eq!(parse_list("a,b c|d"), vec!["a", "b", "c", "d"]);
        assert_eq!(
            parse_json(r#"{"ok":true}"#).and_then(|value| value.get("ok").and_then(Value::as_bool)),
            Some(true)
        );
        assert!(parse_json("{bad json").is_none());
        assert!(parse_json_collection(r#"[1,2]"#).is_some());
        assert!(parse_json_collection("123").is_none());
        assert_eq!(
            parse_key_values("a=1\nb: 2, =skip"),
            vec![
                KeyValue {
                    key: "a".to_owned(),
                    value: "1".to_owned(),
                },
                KeyValue {
                    key: "b".to_owned(),
                    value: "2".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn form_reports_required_and_type_errors() -> std::result::Result<(), Box<dyn StdError>> {
        let form = form([("page", "abc"), ("name", " ")]);
        let required = match form.require::<String>("name") {
            Ok(value) => return Err(format!("expected required error, got {value}").into()),
            Err(error) => error,
        };
        let typed = match form.get::<u32>("page") {
            Ok(value) => return Err(format!("expected type error, got {value:?}").into()),
            Err(error) => error,
        };

        match required.kind() {
            ErrorKind::Required { operation, key } => {
                assert_eq!(*operation, "require");
                assert_eq!(key, "name");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }
        match typed.kind() {
            ErrorKind::Type { operation, key, .. } => {
                assert_eq!(*operation, "get");
                assert_eq!(key, "page");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }
        Ok(())
    }

    #[test]
    fn counter_counts_and_keeps_tie_order() {
        let counts = counter(["a", "b", "a", "c", "b"]);

        assert_eq!(counts.get("a"), 2);
        assert_eq!(counts.get("missing"), 0);
        assert_eq!(counts.total(), 5);
        assert_eq!(counts.len(), 3);
        assert!(!counts.is_empty());
        assert_eq!(counts.most_common(), vec![("a", 2), ("b", 2), ("c", 1)]);
    }

    #[test]
    fn empty_counter_reports_empty() {
        let counts = counter(Vec::<&str>::new());

        assert_eq!(counts.total(), 0);
        assert_eq!(counts.len(), 0);
        assert!(counts.is_empty());
        assert_eq!(counts.most_common(), Vec::<(&str, usize)>::new());
    }

    #[test]
    fn group_by_groups_items_and_preserves_group_order() {
        let users = vec![
            User {
                name: "Ada",
                role: "admin",
            },
            User {
                name: "Grace",
                role: "user",
            },
            User {
                name: "Linus",
                role: "admin",
            },
        ];

        let groups = group_by(users, |user| user.role);
        let admins: Vec<&str> = groups["admin"].iter().map(|user| user.name).collect();
        let regular_users: Vec<&str> = groups["user"].iter().map(|user| user.name).collect();

        assert_eq!(admins, vec!["Ada", "Linus"]);
        assert_eq!(regular_users, vec!["Grace"]);
    }

    #[test]
    fn chunked_keeps_tail_chunk() -> Result<()> {
        let chunks = chunked([1, 2, 3, 4, 5], 2)?;

        assert_eq!(chunks, vec![vec![1, 2], vec![3, 4], vec![5]]);
        Ok(())
    }

    #[test]
    fn chunked_rejects_zero_size() -> std::result::Result<(), Box<dyn StdError>> {
        let error = match chunked([1, 2, 3], 0) {
            Ok(_) => return Err("expected invalid size error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::InvalidSize { operation, size } => {
                assert_eq!(*operation, "chunked");
                assert_eq!(*size, 0);
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }
}
