//! 极简 SQLite API。
//!
//! 这个模块只封装 SQLite，提供同步、高层、动态行 API。普通用法只需要打开数据库、
//! 执行 SQL、查询行和读取字段。

use std::{
    collections::HashSet,
    error::Error as StdError,
    fmt, fs as std_fs,
    sync::{Mutex, MutexGuard},
};

use rusqlite::{Connection, params_from_iter, types::ValueRef};

use crate::fs::Path as FsPath;

const SQL_PREVIEW_CHARS: usize = 500;

/// 创建 SQLite 参数列表的宏。
///
/// 适合传给 [`Database::execute_params`]、[`Database::query_params`] 和
/// [`Database::get_params`]，例如 `sqlite::params![1, "Ada"]`。
pub use crate::__easy_rust_sqlite_params as params;

/// sqlite 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。常见写法是 `let db = sqlite::open("app.db")?;`。
pub type Result<T> = std::result::Result<T, Error>;

/// sqlite 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分打开、执行、查询或结果形状错误时，
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

/// sqlite 模块的具体错误原因。
///
/// 错误信息会包含操作名和关键上下文：路径、SQL 预览或结果形状说明。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 打开 SQLite 数据库失败。
    #[error("sqlite open `{path}` failed")]
    Open {
        /// 发生错误的数据库路径。
        path: FsPath,
    },

    /// 创建数据库父目录失败。
    #[error("sqlite create_dir `{path}` failed")]
    CreateDir {
        /// 发生错误的目录路径。
        path: FsPath,
    },

    /// 执行 SQL 失败。
    #[error("sqlite execute `{sql}` failed")]
    Execute {
        /// SQL 文本预览。
        sql: String,
    },

    /// 查询 SQL 失败。
    #[error("sqlite query `{sql}` failed")]
    Query {
        /// SQL 文本预览。
        sql: String,
    },

    /// 查询结果形状不符合高层 API 要求。
    #[error("sqlite {operation} `{sql}` failed: {message}")]
    Shape {
        /// 发生错误的操作名，例如 `query`。
        operation: &'static str,
        /// SQL 文本预览。
        sql: String,
        /// 面向人的形状错误说明。
        message: String,
    },

    /// 查询结果列类型不符合读取方法要求。
    #[error("sqlite {operation} column `{column}` failed: expected {expected}, got {actual}")]
    ColumnType {
        /// 发生错误的操作名，例如 `row.text`。
        operation: &'static str,
        /// 发生错误的列名。
        column: String,
        /// 读取方法期望的 SQLite 类型。
        expected: &'static str,
        /// 实际读取到的 SQLite 类型。
        actual: &'static str,
    },
}

/// SQLite 参数列表。
///
/// 通常不需要手动创建这个类型；使用 [`params!`] 宏即可，例如 `sqlite::params![1, "Ada"]`。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Params {
    values: Vec<Value>,
}

impl Params {
    fn into_rusqlite_values(self) -> Vec<rusqlite::types::Value> {
        self.values.into_iter().map(into_rusqlite_value).collect()
    }
}

#[doc(hidden)]
impl FromIterator<Value> for Params {
    fn from_iter<T: IntoIterator<Item = Value>>(iter: T) -> Self {
        Self {
            values: iter.into_iter().collect(),
        }
    }
}

/// SQLite 基础值。
///
/// 查询结果和参数都使用这个类型表示 SQLite 的基础类型：空值、整数、浮点、文本和二进制。
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    /// SQLite `NULL`。
    Null,
    /// SQLite 整数。
    Integer(i64),
    /// SQLite 浮点数。
    Real(f64),
    /// SQLite 文本。
    Text(String),
    /// SQLite 二进制。
    Blob(Vec<u8>),
}

impl Value {
    /// 如果值是整数，返回 `i64`。
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Integer(value) => Some(*value),
            Self::Null | Self::Real(_) | Self::Text(_) | Self::Blob(_) => None,
        }
    }

    /// 如果值是浮点数，返回 `f64`。
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Real(value) => Some(*value),
            Self::Null | Self::Integer(_) | Self::Text(_) | Self::Blob(_) => None,
        }
    }

    /// 如果值是文本，返回字符串切片。
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::Text(value) => Some(value),
            Self::Null | Self::Integer(_) | Self::Real(_) | Self::Blob(_) => None,
        }
    }

    /// 如果值是二进制，返回字节切片。
    #[must_use]
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Self::Blob(value) => Some(value),
            Self::Null | Self::Integer(_) | Self::Real(_) | Self::Text(_) => None,
        }
    }

    /// 判断值是否是 SQLite `NULL`。
    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    fn sqlite_type_name(&self) -> &'static str {
        match self {
            Self::Null => "null",
            Self::Integer(_) => "integer",
            Self::Real(_) => "real",
            Self::Text(_) => "text",
            Self::Blob(_) => "blob",
        }
    }
}

impl From<i8> for Value {
    fn from(value: i8) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<i16> for Value {
    fn from(value: i16) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<i32> for Value {
    fn from(value: i32) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<i64> for Value {
    fn from(value: i64) -> Self {
        Self::Integer(value)
    }
}

impl From<isize> for Value {
    fn from(value: isize) -> Self {
        Self::Integer(value as i64)
    }
}

impl From<u8> for Value {
    fn from(value: u8) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<u16> for Value {
    fn from(value: u16) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<u32> for Value {
    fn from(value: u32) -> Self {
        Self::Integer(i64::from(value))
    }
}

impl From<f32> for Value {
    fn from(value: f32) -> Self {
        Self::Real(f64::from(value))
    }
}

impl From<f64> for Value {
    fn from(value: f64) -> Self {
        Self::Real(value)
    }
}

impl From<bool> for Value {
    fn from(value: bool) -> Self {
        Self::Integer(if value { 1 } else { 0 })
    }
}

impl From<String> for Value {
    fn from(value: String) -> Self {
        Self::Text(value)
    }
}

impl From<&String> for Value {
    fn from(value: &String) -> Self {
        Self::Text(value.clone())
    }
}

impl From<&str> for Value {
    fn from(value: &str) -> Self {
        Self::Text(value.to_owned())
    }
}

impl From<Vec<u8>> for Value {
    fn from(value: Vec<u8>) -> Self {
        Self::Blob(value)
    }
}

impl From<&Vec<u8>> for Value {
    fn from(value: &Vec<u8>) -> Self {
        Self::Blob(value.clone())
    }
}

impl From<&[u8]> for Value {
    fn from(value: &[u8]) -> Self {
        Self::Blob(value.to_vec())
    }
}

impl<T> From<Option<T>> for Value
where
    Value: From<T>,
{
    fn from(value: Option<T>) -> Self {
        match value {
            Some(value) => Self::from(value),
            None => Self::Null,
        }
    }
}

fn into_rusqlite_value(value: Value) -> rusqlite::types::Value {
    match value {
        Value::Null => rusqlite::types::Value::Null,
        Value::Integer(value) => rusqlite::types::Value::Integer(value),
        Value::Real(value) => rusqlite::types::Value::Real(value),
        Value::Text(value) => rusqlite::types::Value::Text(value),
        Value::Blob(value) => rusqlite::types::Value::Blob(value),
    }
}

/// SQLite 查询结果行。
///
/// 行按查询列顺序保存字段。列名重复时查询会直接返回错误，避免 `get` 静默覆盖字段。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Row {
    fields: Vec<(String, Value)>,
}

impl Row {
    /// 读取指定列的值。
    ///
    /// 列不存在时返回 `None`。如果只是读取常见类型，优先使用 [`Row::text`]、
    /// [`Row::int`]、[`Row::float`]、[`Row::bytes`] 或 [`Row::is_null`]。
    #[must_use]
    pub fn get(&self, name: impl AsRef<str>) -> Option<&Value> {
        let name = name.as_ref();
        self.fields
            .iter()
            .find(|(column, _)| column == name)
            .map(|(_, value)| value)
    }

    /// 按文本读取指定列。
    ///
    /// 列不存在或值是 `NULL` 时返回 `Ok(None)`；如果列存在但不是文本，返回
    /// [`ErrorKind::ColumnType`]，错误里包含操作名和列名。
    pub fn text(&self, name: impl AsRef<str>) -> Result<Option<&str>> {
        let name = name.as_ref();
        match self.get(name) {
            Some(Value::Text(value)) => Ok(Some(value.as_str())),
            Some(Value::Null) | None => Ok(None),
            Some(value) => Err(column_type_error(
                "row.text",
                name,
                "text",
                value.sqlite_type_name(),
            )),
        }
    }

    /// 按整数读取指定列。
    ///
    /// 列不存在或值是 `NULL` 时返回 `Ok(None)`；如果列存在但不是整数，返回
    /// [`ErrorKind::ColumnType`]，错误里包含操作名和列名。
    pub fn int(&self, name: impl AsRef<str>) -> Result<Option<i64>> {
        let name = name.as_ref();
        match self.get(name) {
            Some(Value::Integer(value)) => Ok(Some(*value)),
            Some(Value::Null) | None => Ok(None),
            Some(value) => Err(column_type_error(
                "row.int",
                name,
                "integer",
                value.sqlite_type_name(),
            )),
        }
    }

    /// 按浮点数读取指定列。
    ///
    /// SQLite 整数会自动转换为 `f64`。列不存在或值是 `NULL` 时返回 `Ok(None)`；
    /// 如果列存在但不是整数或浮点数，返回 [`ErrorKind::ColumnType`]。
    pub fn float(&self, name: impl AsRef<str>) -> Result<Option<f64>> {
        let name = name.as_ref();
        match self.get(name) {
            Some(Value::Integer(value)) => Ok(Some(*value as f64)),
            Some(Value::Real(value)) => Ok(Some(*value)),
            Some(Value::Null) | None => Ok(None),
            Some(value) => Err(column_type_error(
                "row.float",
                name,
                "integer or real",
                value.sqlite_type_name(),
            )),
        }
    }

    /// 按字节读取指定列。
    ///
    /// 列不存在或值是 `NULL` 时返回 `Ok(None)`；如果列存在但不是二进制，返回
    /// [`ErrorKind::ColumnType`]，错误里包含操作名和列名。
    pub fn bytes(&self, name: impl AsRef<str>) -> Result<Option<&[u8]>> {
        let name = name.as_ref();
        match self.get(name) {
            Some(Value::Blob(value)) => Ok(Some(value.as_slice())),
            Some(Value::Null) | None => Ok(None),
            Some(value) => Err(column_type_error(
                "row.bytes",
                name,
                "blob",
                value.sqlite_type_name(),
            )),
        }
    }

    /// 判断指定列是否是 SQLite `NULL`。
    ///
    /// 只有列存在且值为 `NULL` 时返回 `true`；列不存在时返回 `false`。
    #[must_use]
    pub fn is_null(&self, name: impl AsRef<str>) -> bool {
        self.get(name).is_some_and(Value::is_null)
    }

    /// 返回当前行的列名。
    ///
    /// 列名按 SQL 查询结果顺序返回。
    #[must_use]
    pub fn columns(&self) -> Vec<&str> {
        self.fields
            .iter()
            .map(|(column, _)| column.as_str())
            .collect()
    }

    /// 返回列数量。
    #[must_use]
    pub fn len(&self) -> usize {
        self.fields.len()
    }

    /// 判断当前行是否没有任何列。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

/// SQLite 数据库连接。
///
/// 这个类型内部串行保护 SQLite 连接，方法都接收 `&self`，方便在应用中共享使用。
pub struct Database {
    connection: Mutex<Connection>,
}

impl Database {
    /// 执行不带参数的 SQL。
    ///
    /// 返回 SQLite 报告的影响行数。SQL 错误会返回 [`ErrorKind::Execute`]。
    pub fn execute(&self, sql: impl AsRef<str>) -> Result<usize> {
        self.execute_params(sql, Params::default())
    }

    /// 执行带参数的 SQL。
    ///
    /// 参数请使用 [`params!`] 创建，例如 `sqlite::params![1, "Ada"]`。
    pub fn execute_params(&self, sql: impl AsRef<str>, params: Params) -> Result<usize> {
        let sql = sql.as_ref();
        let values = params.into_rusqlite_values();
        self.lock_connection()
            .execute(sql, params_from_iter(values.iter()))
            .map_err(|source| execute_error(sql, source))
    }

    /// 查询不带参数的 SQL，并返回所有结果行。
    ///
    /// 查询错误会返回 [`ErrorKind::Query`]；重复列名会返回 [`ErrorKind::Shape`]。
    pub fn query(&self, sql: impl AsRef<str>) -> Result<Vec<Row>> {
        self.query_params(sql, Params::default())
    }

    /// 查询带参数的 SQL，并返回所有结果行。
    ///
    /// 参数请使用 [`params!`] 创建。结果行使用动态 [`Row`] 表示。
    pub fn query_params(&self, sql: impl AsRef<str>, params: Params) -> Result<Vec<Row>> {
        self.query_inner(sql.as_ref(), params, None)
    }

    /// 查询不带参数的 SQL，并返回第一行。
    ///
    /// 没有结果时返回 `Ok(None)`。
    pub fn get(&self, sql: impl AsRef<str>) -> Result<Option<Row>> {
        self.get_params(sql, Params::default())
    }

    /// 查询带参数的 SQL，并返回第一行。
    ///
    /// 没有结果时返回 `Ok(None)`；参数请使用 [`params!`] 创建。
    pub fn get_params(&self, sql: impl AsRef<str>, params: Params) -> Result<Option<Row>> {
        Ok(self
            .query_inner(sql.as_ref(), params, Some(1))?
            .into_iter()
            .next())
    }

    fn query_inner(&self, sql: &str, params: Params, limit: Option<usize>) -> Result<Vec<Row>> {
        let values = params.into_rusqlite_values();
        let connection = self.lock_connection();
        let mut statement = connection
            .prepare(sql)
            .map_err(|source| query_error(sql, source))?;
        let column_names: Vec<String> = statement
            .column_names()
            .into_iter()
            .map(ToOwned::to_owned)
            .collect();
        validate_unique_columns("query", sql, &column_names)?;

        let mut rows = statement
            .query(params_from_iter(values.iter()))
            .map_err(|source| query_error(sql, source))?;
        let mut output = Vec::new();

        while let Some(row) = rows.next().map_err(|source| query_error(sql, source))? {
            output.push(row_from_sqlite_row(row, &column_names, sql)?);
            if limit.is_some_and(|limit| output.len() >= limit) {
                break;
            }
        }

        Ok(output)
    }

    fn lock_connection(&self) -> MutexGuard<'_, Connection> {
        match self.connection.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }
}

/// 打开 SQLite 数据库文件。
///
/// 路径的父目录会自动创建。打开失败会返回 [`ErrorKind::Open`]。
pub fn open(path: impl Into<FsPath>) -> Result<Database> {
    let path = path.into();
    create_parent_dirs(&path)?;
    let connection = Connection::open(path.as_std_path())
        .map_err(|source| Error::with_source(ErrorKind::Open { path: path.clone() }, source))?;

    Ok(Database {
        connection: Mutex::new(connection),
    })
}

/// 创建内存 SQLite 数据库。
///
/// 适合测试、脚本临时处理和不需要落盘的小任务。
pub fn memory() -> Result<Database> {
    let path = FsPath::from(":memory:");
    let connection = Connection::open_in_memory()
        .map_err(|source| Error::with_source(ErrorKind::Open { path }, source))?;

    Ok(Database {
        connection: Mutex::new(connection),
    })
}

fn row_from_sqlite_row(row: &rusqlite::Row<'_>, columns: &[String], sql: &str) -> Result<Row> {
    let mut fields = Vec::with_capacity(columns.len());

    for (index, column) in columns.iter().enumerate() {
        let value = row
            .get_ref(index)
            .map_err(|source| query_error(sql, source))?;
        fields.push((column.clone(), value_from_ref(value)));
    }

    Ok(Row { fields })
}

fn value_from_ref(value: ValueRef<'_>) -> Value {
    match value {
        ValueRef::Null => Value::Null,
        ValueRef::Integer(value) => Value::Integer(value),
        ValueRef::Real(value) => Value::Real(value),
        ValueRef::Text(value) => Value::Text(String::from_utf8_lossy(value).into_owned()),
        ValueRef::Blob(value) => Value::Blob(value.to_vec()),
    }
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

fn validate_unique_columns(operation: &'static str, sql: &str, columns: &[String]) -> Result<()> {
    let mut seen = HashSet::new();

    for column in columns {
        if !seen.insert(column.clone()) {
            return Err(ErrorKind::Shape {
                operation,
                sql: sql_preview(sql),
                message: format!("duplicate column `{column}`"),
            }
            .into());
        }
    }

    Ok(())
}

fn column_type_error(
    operation: &'static str,
    column: &str,
    expected: &'static str,
    actual: &'static str,
) -> Error {
    ErrorKind::ColumnType {
        operation,
        column: column.to_owned(),
        expected,
        actual,
    }
    .into()
}

fn execute_error(sql: &str, source: rusqlite::Error) -> Error {
    Error::with_source(
        ErrorKind::Execute {
            sql: sql_preview(sql),
        },
        source,
    )
}

fn query_error(sql: &str, source: rusqlite::Error) -> Error {
    Error::with_source(
        ErrorKind::Query {
            sql: sql_preview(sql),
        },
        source,
    )
}

fn sql_preview(sql: &str) -> String {
    let mut output = String::new();
    let mut chars = sql.chars();

    for _ in 0..SQL_PREVIEW_CHARS {
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

#[doc(hidden)]
#[macro_export]
macro_rules! __easy_rust_sqlite_params {
    ($($value:expr),* $(,)?) => {
        vec![$($crate::sqlite::Value::from($value)),*]
            .into_iter()
            .collect::<$crate::sqlite::Params>()
    };
}

#[cfg(test)]
mod tests {
    use std::{
        error::Error as StdError,
        fs as test_fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    fn temp_root(test_name: &str) -> std::result::Result<PathBuf, Box<dyn StdError>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = std::env::temp_dir().join(format!(
            "easy-rust-sqlite-{}-{test_name}-{nanos}",
            std::process::id()
        ));
        test_fs::create_dir_all(&root)?;
        Ok(root)
    }

    #[test]
    fn memory_creates_inserts_and_queries_rows() -> std::result::Result<(), Box<dyn StdError>> {
        let db = memory()?;

        db.execute("create table users (id integer, name text)")?;
        let changed = db.execute_params("insert into users values (?, ?)", params![1, "Ada"])?;
        let rows = db.query("select id, name from users")?;

        assert_eq!(changed, 1);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].columns(), vec!["id", "name"]);
        assert_eq!(rows[0].int("id")?, Some(1));
        assert_eq!(rows[0].text("name")?, Some("Ada"));
        Ok(())
    }

    #[test]
    fn open_creates_parent_directories() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("open")?;
        let path = root.join("nested/app.db");

        let db = open(path.display().to_string())?;
        db.execute("create table items (id integer)")?;

        assert!(path.exists());
        Ok(())
    }

    #[test]
    fn execute_returns_changed_rows() -> std::result::Result<(), Box<dyn StdError>> {
        let db = memory()?;

        db.execute("create table users (id integer, name text)")?;
        db.execute_params("insert into users values (?, ?)", params![1, "Ada"])?;
        db.execute_params("insert into users values (?, ?)", params![2, "Grace"])?;
        let changed = db.execute("update users set name = 'Updated'")?;

        assert_eq!(changed, 2);
        Ok(())
    }

    #[test]
    fn get_returns_none_when_no_row_exists() -> std::result::Result<(), Box<dyn StdError>> {
        let db = memory()?;

        db.execute("create table users (id integer)")?;

        assert_eq!(db.get("select id from users")?, None);
        Ok(())
    }

    #[test]
    fn get_params_returns_first_matching_row() -> std::result::Result<(), Box<dyn StdError>> {
        let db = memory()?;

        db.execute("create table users (id integer, name text)")?;
        db.execute_params("insert into users values (?, ?)", params![1, "Ada"])?;
        let row = db
            .get_params("select id, name from users where id = ?", params![1])?
            .ok_or("missing row")?;

        assert_eq!(row.text("name")?, Some("Ada"));
        Ok(())
    }

    #[test]
    fn params_support_common_sqlite_values() -> std::result::Result<(), Box<dyn StdError>> {
        let db = memory()?;

        db.execute(
            "create table values_table (
                n integer,
                r real,
                t text,
                b blob,
                flag integer,
                none text,
                some integer,
                explicit_null text
            )",
        )?;
        db.execute_params(
            "insert into values_table values (?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                7,
                1.5,
                "Ada",
                vec![1_u8, 2, 3],
                true,
                Option::<i32>::None,
                Some(9),
                Value::Null
            ],
        )?;
        let row = db
            .get("select n, r, t, b, flag, none, some, explicit_null from values_table")?
            .ok_or("missing row")?;

        assert_eq!(row.int("n")?, Some(7));
        assert_eq!(row.float("n")?, Some(7.0));
        assert_eq!(row.float("r")?, Some(1.5));
        assert_eq!(row.text("t")?, Some("Ada"));
        assert_eq!(row.bytes("b")?, Some(&[1, 2, 3][..]));
        assert_eq!(row.int("flag")?, Some(1));
        assert!(row.is_null("none"));
        assert_eq!(row.int("some")?, Some(9));
        assert!(row.is_null("explicit_null"));
        assert_eq!(row.text("missing")?, None);
        assert_eq!(row.int("none")?, None);
        Ok(())
    }

    #[test]
    fn row_typed_helpers_return_column_type_errors() -> std::result::Result<(), Box<dyn StdError>> {
        let db = memory()?;

        db.execute("create table users (id integer, name text)")?;
        db.execute_params("insert into users values (?, ?)", params![1, "Ada"])?;
        let row = db.get("select id, name from users")?.ok_or("missing row")?;

        let error = match row.int("name") {
            Ok(value) => return Err(format!("expected type error, got {value:?}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::ColumnType {
                operation,
                column,
                expected,
                actual,
            } => {
                assert_eq!(*operation, "row.int");
                assert_eq!(column, "name");
                assert_eq!(*expected, "integer");
                assert_eq!(*actual, "text");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        let message = error.to_string();
        assert!(message.contains("row.int"));
        assert!(message.contains("name"));
        Ok(())
    }

    #[test]
    fn duplicate_column_names_return_shape_error() -> std::result::Result<(), Box<dyn StdError>> {
        let db = memory()?;

        let error = match db.query("select 1 as id, 2 as id") {
            Ok(rows) => return Err(format!("expected shape error, got {rows:?}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Shape {
                operation,
                sql,
                message,
            } => {
                assert_eq!(*operation, "query");
                assert_eq!(sql, "select 1 as id, 2 as id");
                assert!(message.contains("duplicate column"));
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[test]
    fn execute_errors_include_sql_preview() -> std::result::Result<(), Box<dyn StdError>> {
        let db = memory()?;

        let error = match db.execute("insert into missing values (1)") {
            Ok(changed) => return Err(format!("expected execute error, got {changed}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Execute { sql, .. } => assert_eq!(sql, "insert into missing values (1)"),
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[test]
    fn query_errors_include_sql_preview() -> std::result::Result<(), Box<dyn StdError>> {
        let db = memory()?;

        let error = match db.query("select * from missing") {
            Ok(rows) => return Err(format!("expected query error, got {rows:?}").into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Query { sql, .. } => assert_eq!(sql, "select * from missing"),
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }
}
