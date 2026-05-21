//! 极简 CSV 文件 API。
//!
//! 这个模块提供像 Python 一样直接读写 CSV 文件的高层入口。默认 CSV 带 header，
//! 强类型读写使用 serde，动态行读写使用 [`Row`]。

use std::{collections::HashSet, error::Error as StdError, fmt, fs as std_fs, io};

use serde::{Serialize, de::DeserializeOwned};

use crate::fs::Path as FsPath;

/// csv 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。常见写法是 `let rows = csv::read_rows(path)?;`。
pub type Result<T> = std::result::Result<T, Error>;

/// csv 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分读取、写入、目录创建或 CSV 形状错误时，
/// 使用 [`Error::kind`]。
#[derive(Debug)]
pub struct Error {
    kind: Box<ErrorKind>,
    source: Option<Box<dyn StdError + Send + Sync + 'static>>,
}

impl Error {
    fn new(kind: ErrorKind) -> Self {
        Self {
            kind: Box::new(kind),
            source: None,
        }
    }

    fn with_source(kind: ErrorKind, source: impl StdError + Send + Sync + 'static) -> Self {
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
        self.source
            .as_deref()
            .map(|source| source as &(dyn StdError + 'static))
    }
}

/// csv 模块的具体错误原因。
///
/// 每个错误都带有操作名和路径，方便定位 CSV 读写失败的位置。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 读取 CSV 失败。
    #[error("csv read `{path}` failed")]
    Read {
        /// 发生错误的路径。
        path: FsPath,
    },

    /// 写入 CSV 失败。
    #[error("csv write `{path}` failed")]
    Write {
        /// 发生错误的路径。
        path: FsPath,
    },

    /// 创建 CSV 文件父目录失败。
    #[error("csv create_dir `{path}` failed")]
    CreateDir {
        /// 发生错误的目录路径。
        path: FsPath,
    },

    /// CSV 数据形状不符合高层 API 要求。
    #[error("csv {operation} `{path}` failed: {message}")]
    Shape {
        /// 发生错误的操作名，例如 `read_rows`。
        operation: &'static str,
        /// 发生错误的路径。
        path: FsPath,
        /// 面向人的形状错误说明。
        message: String,
    },
}

/// 动态 CSV 行。
///
/// 当 CSV 结构不固定，或只需要像 Python `dict` 一样读写普通行数据时，使用这个类型。
/// 字段值统一按字符串保存，并保留字段首次出现顺序。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Row {
    fields: Vec<(String, String)>,
}

impl Row {
    /// 创建空行。
    ///
    /// 适合手动拼出动态 CSV 行，再交给 [`write_rows`] 写入文件。
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 从键值对创建动态行。
    ///
    /// 重复 key 会覆盖旧值，但保留第一次出现的位置。
    #[must_use]
    pub fn from_pairs<K, V>(pairs: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: AsRef<str>,
        V: ToString,
    {
        let mut row = Self::new();
        for (key, value) in pairs {
            row.set(key, value);
        }
        row
    }

    /// 读取字段值。
    ///
    /// 字段不存在时返回 `None`。字段存在但 CSV 中为空字符串时返回 `Some("")`。
    #[must_use]
    pub fn get(&self, key: impl AsRef<str>) -> Option<&str> {
        let key = key.as_ref();
        self.fields
            .iter()
            .find(|(field, _)| field == key)
            .map(|(_, value)| value.as_str())
    }

    /// 设置字段值并返回自身。
    ///
    /// 已有字段会覆盖值并保留原位置；新字段会追加到行尾，影响 [`write_rows`] 收集 header 的顺序。
    pub fn set(&mut self, key: impl AsRef<str>, value: impl ToString) -> &mut Self {
        let key = key.as_ref();
        let value = value.to_string();

        if let Some((_, existing)) = self.fields.iter_mut().find(|(field, _)| field == key) {
            *existing = value;
        } else {
            self.fields.push((key.to_owned(), value));
        }

        self
    }

    /// 返回当前行的字段名。
    ///
    /// 字段名按首次出现顺序返回，适合检查动态行结构。
    #[must_use]
    pub fn headers(&self) -> Vec<&str> {
        self.fields
            .iter()
            .map(|(field, _)| field.as_str())
            .collect()
    }

    /// 返回字段数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// 判断当前行是否没有任何字段。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// 读取带 header 的 CSV 文件，并反序列化为结构体列表。
///
/// 类型 `T` 需要实现 serde 反序列化。CSV 第一行会作为 header 使用，读取或解析失败会返回
/// [`ErrorKind::Read`]。
pub fn read<T>(path: impl Into<FsPath>) -> Result<Vec<T>>
where
    T: DeserializeOwned,
{
    let path = path.into();
    let mut reader = csv_crate::ReaderBuilder::new()
        .has_headers(true)
        .from_path(path.as_std_path())
        .map_err(|source| read_error(&path, source))?;

    let mut rows = Vec::new();
    for record in reader.deserialize() {
        rows.push(record.map_err(|source| read_error(&path, source))?);
    }

    Ok(rows)
}

/// 把结构体列表写入带 header 的 CSV 文件。
///
/// 类型 `T` 需要实现 serde 序列化。写入会自动创建父目录；空列表会创建空文件。
pub fn write<T>(path: impl Into<FsPath>, rows: &[T]) -> Result<()>
where
    T: Serialize,
{
    let path = path.into();
    create_parent_dirs(&path)?;

    let mut writer = csv_crate::WriterBuilder::new()
        .has_headers(true)
        .from_path(path.as_std_path())
        .map_err(|source| write_error(&path, source))?;

    for row in rows {
        writer
            .serialize(row)
            .map_err(|source| write_error(&path, source))?;
    }

    writer
        .flush()
        .map_err(|source| write_error(&path, source.into()))?;
    Ok(())
}

/// 追加结构体行到带 header 的 CSV 文件。
///
/// 文件不存在或为空时会写入 header；已有内容时只追加数据行，不重复写 header。
pub fn append<T>(path: impl Into<FsPath>, rows: &[T]) -> Result<()>
where
    T: Serialize,
{
    let path = path.into();
    create_parent_dirs(&path)?;
    let has_content = file_has_content(&path)?;
    let file = std_fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_std_path())
        .map_err(|source| write_error(&path, source.into()))?;
    let mut writer = csv_crate::WriterBuilder::new()
        .has_headers(!has_content)
        .from_writer(file);

    for row in rows {
        writer
            .serialize(row)
            .map_err(|source| write_error(&path, source))?;
    }

    writer
        .flush()
        .map_err(|source| write_error(&path, source.into()))?;
    Ok(())
}

/// 读取带 header 的 CSV 文件为动态行。
///
/// 所有字段值都会按字符串保存。重复 header 会返回 [`ErrorKind::Shape`]，避免动态行读取时
/// 静默覆盖字段。
pub fn read_rows(path: impl Into<FsPath>) -> Result<Vec<Row>> {
    let path = path.into();
    let mut reader = csv_crate::ReaderBuilder::new()
        .has_headers(true)
        .from_path(path.as_std_path())
        .map_err(|source| read_error(&path, source))?;

    let headers = reader
        .headers()
        .map_err(|source| read_error(&path, source))?
        .clone();
    validate_unique_headers(&path, &headers)?;

    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|source| read_error(&path, source))?;
        let mut row = Row::new();

        for (header, value) in headers.iter().zip(record.iter()) {
            row.set(header, value);
        }

        rows.push(row);
    }

    Ok(rows)
}

/// 把动态行写入带 header 的 CSV 文件。
///
/// header 会从所有行中按首次出现顺序收集；某行缺少某个字段时写为空字符串。写入会自动创建
/// 父目录，空行列表会创建空文件。
pub fn write_rows(path: impl Into<FsPath>, rows: &[Row]) -> Result<()> {
    let path = path.into();
    create_parent_dirs(&path)?;

    let mut writer = csv_crate::WriterBuilder::new()
        .has_headers(false)
        .from_path(path.as_std_path())
        .map_err(|source| write_error(&path, source))?;
    let headers = collect_headers(rows);

    if headers.is_empty() {
        writer
            .flush()
            .map_err(|source| write_error(&path, source.into()))?;
        return Ok(());
    }

    writer
        .write_record(&headers)
        .map_err(|source| write_error(&path, source))?;

    for row in rows {
        let record: Vec<&str> = headers
            .iter()
            .map(|header| row.get(header).unwrap_or(""))
            .collect();
        writer
            .write_record(record)
            .map_err(|source| write_error(&path, source))?;
    }

    writer
        .flush()
        .map_err(|source| write_error(&path, source.into()))?;
    Ok(())
}

/// 追加动态行到带 header 的 CSV 文件。
///
/// 文件不存在或为空时会根据追加行写入 header；已有内容时沿用现有 header。行里出现现有
/// header 之外的新字段会返回 [`ErrorKind::Shape`]，避免静默丢列。
pub fn append_rows(path: impl Into<FsPath>, rows: &[Row]) -> Result<()> {
    let path = path.into();
    create_parent_dirs(&path)?;
    let has_content = file_has_content(&path)?;
    let headers = if has_content {
        let headers = read_existing_headers(&path)?;
        validate_rows_fit_headers(&path, &headers, rows)?;
        headers
    } else {
        collect_headers(rows)
    };

    let file = std_fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path.as_std_path())
        .map_err(|source| write_error(&path, source.into()))?;
    let mut writer = csv_crate::WriterBuilder::new()
        .has_headers(false)
        .from_writer(file);

    if !has_content && !headers.is_empty() {
        writer
            .write_record(&headers)
            .map_err(|source| write_error(&path, source))?;
    }

    for row in rows {
        let record: Vec<&str> = headers
            .iter()
            .map(|header| row.get(header).unwrap_or(""))
            .collect();
        writer
            .write_record(record)
            .map_err(|source| write_error(&path, source))?;
    }

    writer
        .flush()
        .map_err(|source| write_error(&path, source.into()))?;
    Ok(())
}

fn file_has_content(path: &FsPath) -> Result<bool> {
    match std_fs::metadata(path.as_std_path()) {
        Ok(metadata) => Ok(metadata.len() > 0),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(source) => Err(write_error(path, source.into())),
    }
}

fn read_existing_headers(path: &FsPath) -> Result<Vec<String>> {
    let mut reader = csv_crate::ReaderBuilder::new()
        .has_headers(true)
        .from_path(path.as_std_path())
        .map_err(|source| read_error(path, source))?;
    let headers = reader
        .headers()
        .map_err(|source| read_error(path, source))?
        .clone();
    validate_unique_headers(path, &headers)?;
    Ok(headers.iter().map(ToOwned::to_owned).collect())
}

fn validate_rows_fit_headers(path: &FsPath, headers: &[String], rows: &[Row]) -> Result<()> {
    for row in rows {
        for field in row.headers() {
            if !headers.iter().any(|header| header == field) {
                return Err(ErrorKind::Shape {
                    operation: "append_rows",
                    path: path.clone(),
                    message: format!("字段 `{field}` 不在已有 header 中"),
                }
                .into());
            }
        }
    }
    Ok(())
}

fn create_parent_dirs(path: &FsPath) -> Result<()> {
    if let Some(parent) = path.as_std_path().parent()
        && !parent.as_os_str().is_empty()
    {
        std_fs::create_dir_all(parent).map_err(|source| {
            Error::with_source(
                ErrorKind::CreateDir {
                    path: FsPath::from_std_path(parent),
                },
                source,
            )
        })?;
    }

    Ok(())
}

fn validate_unique_headers(path: &FsPath, headers: &csv_crate::StringRecord) -> Result<()> {
    let mut seen = HashSet::new();

    for header in headers {
        if !seen.insert(header.to_owned()) {
            return Err(ErrorKind::Shape {
                operation: "read_rows",
                path: path.clone(),
                message: format!("duplicate header `{header}`"),
            }
            .into());
        }
    }

    Ok(())
}

fn collect_headers(rows: &[Row]) -> Vec<String> {
    let mut headers = Vec::new();
    let mut seen = HashSet::new();

    for row in rows {
        for header in row.headers() {
            if seen.insert(header.to_owned()) {
                headers.push(header.to_owned());
            }
        }
    }

    headers
}

fn read_error(path: &FsPath, source: csv_crate::Error) -> Error {
    Error::with_source(ErrorKind::Read { path: path.clone() }, source)
}

fn write_error(path: &FsPath, source: csv_crate::Error) -> Error {
    Error::with_source(ErrorKind::Write { path: path.clone() }, source)
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error as StdError,
        fs as test_fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde::{Deserialize, Serialize};

    use super::*;

    #[derive(Debug, Deserialize, Eq, PartialEq, Serialize)]
    struct User {
        id: u64,
        name: String,
    }

    fn temp_root(test_name: &str) -> std::result::Result<PathBuf, Box<dyn StdError>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!(
            "easy-rust-csv-{}-{test_name}-{nanos}",
            std::process::id()
        ));
        test_fs::create_dir_all(&root)?;
        Ok(root)
    }

    fn path_text(path: &std::path::Path) -> String {
        path.display().to_string()
    }

    #[test]
    fn read_deserializes_structs_by_header() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("read")?;
        let path = root.join("users.csv");
        test_fs::write(&path, "id,name\n1,Ada\n2,Grace\n")?;

        let users: Vec<User> = read(path_text(&path))?;

        assert_eq!(
            users,
            vec![
                User {
                    id: 1,
                    name: "Ada".to_owned()
                },
                User {
                    id: 2,
                    name: "Grace".to_owned()
                }
            ]
        );
        Ok(())
    }

    #[test]
    fn write_creates_parent_dirs_and_roundtrips_structs()
    -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("write")?;
        let path = root.join("nested/users.csv");
        let users = vec![
            User {
                id: 1,
                name: "Ada".to_owned(),
            },
            User {
                id: 2,
                name: "Grace".to_owned(),
            },
        ];

        write(path_text(&path), &users)?;
        let read_back: Vec<User> = read(path_text(&path))?;

        assert_eq!(read_back, users);
        Ok(())
    }

    #[test]
    fn read_rows_returns_dynamic_rows() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("read_rows")?;
        let path = root.join("users.csv");
        test_fs::write(&path, "name,age\nAda,36\nGrace,85\n")?;

        let rows = read_rows(path_text(&path))?;

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].headers(), vec!["name", "age"]);
        assert_eq!(rows[0].get("name"), Some("Ada"));
        assert_eq!(rows[0].get("age"), Some("36"));
        Ok(())
    }

    #[test]
    fn row_can_be_built_and_duplicate_keys_update_value()
    -> std::result::Result<(), Box<dyn StdError>> {
        let mut row = Row::new();
        row.set("name", "Ada").set("age", 36).set("name", "Grace");
        let from_pairs = Row::from_pairs([("name", "Ada"), ("name", "Grace")]);

        assert_eq!(row.get("name"), Some("Grace"));
        assert_eq!(row.get("age"), Some("36"));
        assert_eq!(row.headers(), vec!["name", "age"]);
        assert_eq!(row.len(), 2);
        assert!(!row.is_empty());
        assert_eq!(from_pairs.get("name"), Some("Grace"));
        Ok(())
    }

    #[test]
    fn write_rows_collects_headers_and_fills_missing_fields()
    -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("write_rows")?;
        let path = root.join("out/users.csv");
        let rows = vec![
            Row::from_pairs([("name", "Ada"), ("age", "36")]),
            Row::from_pairs([("name", "Grace"), ("city", "London")]),
        ];

        write_rows(path_text(&path), &rows)?;
        let read_back = read_rows(path_text(&path))?;

        assert_eq!(read_back[0].headers(), vec!["name", "age", "city"]);
        assert_eq!(read_back[0].get("city"), Some(""));
        assert_eq!(read_back[1].get("age"), Some(""));
        assert_eq!(read_back[1].get("city"), Some("London"));
        Ok(())
    }

    #[test]
    fn write_rows_empty_list_creates_empty_file() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("empty_rows")?;
        let path = root.join("out/empty.csv");

        write_rows(path_text(&path), &[])?;

        assert_eq!(test_fs::read_to_string(path)?, "");
        Ok(())
    }

    #[test]
    fn append_struct_rows_writes_header_only_for_new_file()
    -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("append")?;
        let path = root.join("out/users.csv");

        append(
            path_text(&path),
            &[User {
                id: 1,
                name: "Ada".to_owned(),
            }],
        )?;
        append(
            path_text(&path),
            &[User {
                id: 2,
                name: "Grace".to_owned(),
            }],
        )?;

        assert_eq!(test_fs::read_to_string(path)?, "id,name\n1,Ada\n2,Grace\n");
        Ok(())
    }

    #[test]
    fn append_rows_uses_existing_headers_and_rejects_new_fields()
    -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("append-rows")?;
        let path = root.join("out/users.csv");

        append_rows(
            path_text(&path),
            &[Row::from_pairs([("name", "Ada"), ("age", "36")])],
        )?;
        append_rows(path_text(&path), &[Row::from_pairs([("name", "Grace")])])?;

        assert_eq!(
            test_fs::read_to_string(&path)?,
            "name,age\nAda,36\nGrace,\n"
        );

        let error = match append_rows(
            path_text(&path),
            &[Row::from_pairs([("name", "Lin"), ("city", "Shanghai")])],
        ) {
            Ok(()) => return Err("expected append_rows shape error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Shape {
                operation, message, ..
            } => {
                assert_eq!(*operation, "append_rows");
                assert!(message.contains("city"));
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        let empty = root.join("out/empty.csv");
        append_rows(path_text(&empty), &[])?;
        assert_eq!(test_fs::read_to_string(empty)?, "");
        Ok(())
    }

    #[test]
    fn read_rows_rejects_duplicate_headers() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("duplicate_headers")?;
        let path = root.join("users.csv");
        test_fs::write(&path, "name,name\nAda,Grace\n")?;

        let error = match read_rows(path_text(&path)) {
            Ok(rows) => return Err(format!("expected duplicate header error, got {rows:?}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Shape {
                operation,
                path: error_path,
                message,
            } => {
                assert_eq!(*operation, "read_rows");
                assert_eq!(error_path.display(), path.display().to_string());
                assert!(message.contains("duplicate header"));
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[test]
    fn csv_format_errors_are_read_errors() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("format_error")?;
        let path = root.join("bad.csv");
        test_fs::write(&path, "name,age\nAda,36,extra\n")?;

        let error = match read_rows(path_text(&path)) {
            Ok(rows) => return Err(format!("expected read error, got {rows:?}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Read {
                path: error_path, ..
            } => {
                assert_eq!(error_path.display(), path.display().to_string());
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }
}
