//! 极简数据集合 API。
//!
//! 这个模块把常见列表、字典、集合、分组、计数和分块操作做成立即返回结果的高层函数。
//! 它不要求用户导入额外能力，也不会返回需要继续组合的惰性对象。

use std::{
    borrow::Borrow,
    collections::{HashMap, HashSet, hash_map::Entry},
    error::Error as StdError,
    fmt,
    hash::Hash,
};

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
        }

        Ok(())
    }
}
