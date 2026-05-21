//! 极简配置 API。
//!
//! 这个模块把 `.env`、真实环境变量和 JSON/TOML/YAML 配置文件合并成一个入口。单个 key 用
//! [`get`]、[`get_or`] 和 [`require`]；完整结构体配置用 [`load`]、[`load_toml`]、
//! [`load_yaml`] 和 [`auto`]。调用方不需要手动管理配置来源。

use std::{
    collections::BTreeMap, env, error::Error as StdError, fmt, fs as std_fs, io,
    path::Path as StdPath,
};

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::fs::Path as FsPath;

const DEFAULT_DOTENV_PATH: &str = ".env";
const DEFAULT_CONFIG_PATH: &str = "config.json";

/// config 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。常见写法是
/// `let port: u16 = config::get_or("PORT", 8000)?;`。
pub type Result<T> = std::result::Result<T, Error>;

/// config 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。需要区分缺失、解析、文件读取等错误时，使用
/// [`Error::kind`]。
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

/// config 模块的具体错误原因。
///
/// 错误信息会包含操作名和 key、路径或行号，方便定位配置失败的位置。解析错误不会在
/// Display 中输出原始环境变量值，避免泄露密钥。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 必需配置项不存在。
    #[error("config require `{key}` failed: missing value")]
    Missing {
        /// 缺失的配置键。
        key: String,
    },

    /// 配置项解析失败。
    #[error("config parse `{key}` failed: expected {expected}")]
    Parse {
        /// 解析失败的配置键。
        key: String,
        /// 期望解析成的 Rust 类型。
        expected: &'static str,
    },

    /// 读取配置文件失败。
    #[error("config read `{path}` failed")]
    Read {
        /// 发生错误的路径。
        path: String,
    },

    /// JSON 配置解析失败。
    #[error("config json_decode `{path}` failed")]
    JsonDecode {
        /// 发生错误的路径。
        path: String,
    },

    /// TOML 配置解析失败。
    #[error("config {operation} toml_decode `{path}` failed")]
    TomlDecode {
        /// 发生错误的操作名，例如 `load_toml`。
        operation: &'static str,
        /// 发生错误的路径。
        path: String,
    },

    /// YAML 配置解析失败。
    #[error("config {operation} yaml_decode `{path}` failed")]
    YamlDecode {
        /// 发生错误的操作名，例如 `load_yaml`。
        operation: &'static str,
        /// 发生错误的路径。
        path: String,
    },

    /// `.env` 文件格式错误。
    #[error("config dotenv_parse `{path}` line {line} failed: {message}")]
    DotenvParse {
        /// 发生错误的 `.env` 路径。
        path: String,
        /// 发生错误的行号，从 1 开始。
        line: usize,
        /// 可直接展示给用户的格式错误说明。
        message: String,
    },

    /// 配置形状不符合要求。
    #[error("config {operation} failed: {message}")]
    Shape {
        /// 发生错误的操作名，例如 `load` 或 `auto`。
        operation: &'static str,
        /// 可直接展示给用户的形状错误说明。
        message: String,
    },
}

/// 读取可选配置项。
///
/// 这个函数读取当前目录的 `config.json`、`.env` 和真实环境变量；覆盖顺序固定为
/// `config.json`、`.env`、真实环境变量。配置项不存在时返回 `Ok(None)`，存在但不能解析成 `T`
/// 时返回 [`ErrorKind::Parse`]。
pub fn get<T>(key: impl AsRef<str>) -> Result<Option<T>>
where
    T: DeserializeOwned,
{
    get_from_sources(
        key.as_ref(),
        StdPath::new(DEFAULT_DOTENV_PATH),
        real_env_values(),
    )
}

/// 读取配置项，缺失时返回默认值。
///
/// 这个函数适合端口、开关、分页大小等有默认值的配置。缺失会返回 `default`，但配置项存在且
/// 解析失败时仍会返回错误，避免把错误配置静默当成默认值。
pub fn get_or<T>(key: impl AsRef<str>, default: T) -> Result<T>
where
    T: DeserializeOwned,
{
    get_or_from_sources(
        key.as_ref(),
        default,
        StdPath::new(DEFAULT_DOTENV_PATH),
        real_env_values(),
    )
}

/// 读取必需配置项。
///
/// 这个函数适合数据库连接串、密钥路径等必须存在的配置。缺失时返回
/// [`ErrorKind::Missing`]，存在但不能解析成 `T` 时返回 [`ErrorKind::Parse`]。
pub fn require<T>(key: impl AsRef<str>) -> Result<T>
where
    T: DeserializeOwned,
{
    require_from_sources(
        key.as_ref(),
        StdPath::new(DEFAULT_DOTENV_PATH),
        real_env_values(),
    )
}

/// 读取 JSON 配置文件，并用 `.env` 和真实环境变量覆盖。
///
/// JSON 文件必须存在，且根必须是 object。覆盖顺序固定为：JSON 文件最低，当前目录 `.env`
/// 覆盖 JSON，真实环境变量最高。需要纯 JSON 文件读写时，使用 `fs::read_json`。
pub fn load<T>(path: impl Into<FsPath>) -> Result<T>
where
    T: DeserializeOwned,
{
    let path = path.into();
    load_from_sources(
        "load",
        Some(path.as_std_path()),
        ConfigFormat::Json,
        true,
        StdPath::new(DEFAULT_DOTENV_PATH),
        real_env_values(),
    )
}

/// 读取 TOML 配置文件，并用 `.env` 和真实环境变量覆盖。
///
/// TOML 文件必须存在，且根必须是 object。覆盖顺序固定为：TOML 文件最低，当前目录 `.env`
/// 覆盖 TOML，真实环境变量最高。
pub fn load_toml<T>(path: impl Into<FsPath>) -> Result<T>
where
    T: DeserializeOwned,
{
    let path = path.into();
    load_from_sources(
        "load_toml",
        Some(path.as_std_path()),
        ConfigFormat::Toml,
        true,
        StdPath::new(DEFAULT_DOTENV_PATH),
        real_env_values(),
    )
}

/// 读取 YAML 配置文件，并用 `.env` 和真实环境变量覆盖。
///
/// YAML 文件必须存在，且根必须是 object。覆盖顺序固定为：YAML 文件最低，当前目录 `.env`
/// 覆盖 YAML，真实环境变量最高。
pub fn load_yaml<T>(path: impl Into<FsPath>) -> Result<T>
where
    T: DeserializeOwned,
{
    let path = path.into();
    load_from_sources(
        "load_yaml",
        Some(path.as_std_path()),
        ConfigFormat::Yaml,
        true,
        StdPath::new(DEFAULT_DOTENV_PATH),
        real_env_values(),
    )
}

/// 自动读取默认配置。
///
/// 这个函数会读取可选 `.env`、可选 `config.json` 和真实环境变量；没有 `config.json` 也可以
/// 只靠环境变量启动。覆盖顺序固定为：`config.json` 最低，`.env` 覆盖文件，真实环境变量最高。
/// 它不会自动搜索 `config.toml` 或 `config.yaml`，需要这些格式时请显式调用 [`load_toml`] 或
/// [`load_yaml`]。
pub fn auto<T>() -> Result<T>
where
    T: DeserializeOwned,
{
    load_from_sources(
        "auto",
        Some(StdPath::new(DEFAULT_CONFIG_PATH)),
        ConfigFormat::Json,
        false,
        StdPath::new(DEFAULT_DOTENV_PATH),
        real_env_values(),
    )
}

#[derive(Clone, Copy, Debug)]
enum ConfigFormat {
    Json,
    Toml,
    Yaml,
}

impl ConfigFormat {
    fn label(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Toml => "TOML",
            Self::Yaml => "YAML",
        }
    }
}

#[derive(Clone, Debug)]
struct SourceValue {
    raw: String,
    quoted: bool,
}

impl SourceValue {
    fn inferred_json(&self) -> Value {
        if self.quoted {
            return Value::String(self.raw.clone());
        }

        serde_json::from_str(&self.raw).unwrap_or_else(|_| Value::String(self.raw.clone()))
    }
}

fn get_from_sources<T>(
    key: &str,
    dotenv_path: &StdPath,
    real_env: impl IntoIterator<Item = (String, String)>,
) -> Result<Option<T>>
where
    T: DeserializeOwned,
{
    get_from_sources_with_config(
        key,
        Some(StdPath::new(DEFAULT_CONFIG_PATH)),
        dotenv_path,
        real_env,
    )
}

fn get_from_sources_with_config<T>(
    key: &str,
    config_path: Option<&StdPath>,
    dotenv_path: &StdPath,
    real_env: impl IntoIterator<Item = (String, String)>,
) -> Result<Option<T>>
where
    T: DeserializeOwned,
{
    let object = load_object_from_sources(
        "get",
        config_path,
        ConfigFormat::Json,
        false,
        dotenv_path,
        real_env,
    )?;
    let field = field_name_from_env_key(key);
    object
        .get(&field)
        .map(|value| parse_config_value(key, value))
        .transpose()
}

fn get_or_from_sources<T>(
    key: &str,
    default: T,
    dotenv_path: &StdPath,
    real_env: impl IntoIterator<Item = (String, String)>,
) -> Result<T>
where
    T: DeserializeOwned,
{
    Ok(get_from_sources(key, dotenv_path, real_env)?.unwrap_or(default))
}

fn require_from_sources<T>(
    key: &str,
    dotenv_path: &StdPath,
    real_env: impl IntoIterator<Item = (String, String)>,
) -> Result<T>
where
    T: DeserializeOwned,
{
    get_from_sources(key, dotenv_path, real_env)?.ok_or_else(|| {
        ErrorKind::Missing {
            key: key.to_owned(),
        }
        .into()
    })
}

fn load_from_sources<T>(
    operation: &'static str,
    config_path: Option<&StdPath>,
    config_format: ConfigFormat,
    config_required: bool,
    dotenv_path: &StdPath,
    real_env: impl IntoIterator<Item = (String, String)>,
) -> Result<T>
where
    T: DeserializeOwned,
{
    let object = load_object_from_sources(
        operation,
        config_path,
        config_format,
        config_required,
        dotenv_path,
        real_env,
    )?;

    serde_json::from_value(Value::Object(object)).map_err(|error| {
        ErrorKind::Shape {
            operation,
            message: error.to_string(),
        }
        .into()
    })
}

fn load_object_from_sources(
    operation: &'static str,
    config_path: Option<&StdPath>,
    config_format: ConfigFormat,
    config_required: bool,
    dotenv_path: &StdPath,
    real_env: impl IntoIterator<Item = (String, String)>,
) -> Result<Map<String, Value>> {
    let mut object = match config_path {
        Some(path) => read_config_object(operation, config_format, path, config_required)?,
        None => Map::new(),
    };

    merge_env_values(&mut object, read_dotenv_optional(dotenv_path)?);
    merge_env_values(&mut object, source_values_from_env(real_env));
    Ok(object)
}

fn parse_config_value<T>(key: &str, value: &Value) -> Result<T>
where
    T: DeserializeOwned,
{
    serde_json::from_value(value.clone()).map_err(|_| {
        ErrorKind::Parse {
            key: key.to_owned(),
            expected: std::any::type_name::<T>(),
        }
        .into()
    })
}

fn read_config_object(
    operation: &'static str,
    format: ConfigFormat,
    path: &StdPath,
    required: bool,
) -> Result<Map<String, Value>> {
    let path_display = display_path(path);
    let text = match std_fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if !required && error.kind() == io::ErrorKind::NotFound => {
            return Ok(Map::new());
        }
        Err(source) => {
            return Err(Error::with_source(
                ErrorKind::Read { path: path_display },
                source,
            ));
        }
    };

    let value = decode_config_value(operation, format, &path_display, &text)?;

    match value {
        Value::Object(object) => Ok(object),
        _ => Err(ErrorKind::Shape {
            operation,
            message: format!("{} 配置根必须是 object", format.label()),
        }
        .into()),
    }
}

fn decode_config_value(
    operation: &'static str,
    format: ConfigFormat,
    path: &str,
    text: &str,
) -> Result<Value> {
    match format {
        ConfigFormat::Json => serde_json::from_str(text).map_err(|source| {
            Error::with_source(
                ErrorKind::JsonDecode {
                    path: path.to_owned(),
                },
                source,
            )
        }),
        ConfigFormat::Toml => toml::from_str(text).map_err(|source| {
            Error::with_source(
                ErrorKind::TomlDecode {
                    operation,
                    path: path.to_owned(),
                },
                source,
            )
        }),
        ConfigFormat::Yaml => yaml_serde::from_str(text).map_err(|source| {
            Error::with_source(
                ErrorKind::YamlDecode {
                    operation,
                    path: path.to_owned(),
                },
                source,
            )
        }),
    }
}

fn read_dotenv_optional(path: &StdPath) -> Result<BTreeMap<String, SourceValue>> {
    let path_display = display_path(path);
    let text = match std_fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(source) => {
            return Err(Error::with_source(
                ErrorKind::Read { path: path_display },
                source,
            ));
        }
    };

    parse_dotenv(&path_display, &text)
}

fn parse_dotenv(path: &str, text: &str) -> Result<BTreeMap<String, SourceValue>> {
    let mut values = BTreeMap::new();

    for (index, raw_line) in text.lines().enumerate() {
        let line_number = index + 1;
        let mut line = raw_line.trim();

        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(rest) = line.strip_prefix("export ") {
            line = rest.trim_start();
        }

        let Some((key, value)) = line.split_once('=') else {
            return Err(dotenv_parse_error(path, line_number, "缺少 `=`"));
        };

        let key = key.trim();
        if !valid_env_key(key) {
            return Err(dotenv_parse_error(path, line_number, "配置键格式不合法"));
        }

        let value = parse_dotenv_value(path, line_number, value.trim())?;
        values.insert(key.to_owned(), value);
    }

    Ok(values)
}

fn parse_dotenv_value(path: &str, line: usize, value: &str) -> Result<SourceValue> {
    let Some(first) = value.chars().next() else {
        return Ok(SourceValue {
            raw: String::new(),
            quoted: false,
        });
    };

    if first != '\'' && first != '"' {
        return Ok(SourceValue {
            raw: value.to_owned(),
            quoted: false,
        });
    }

    if !value.ends_with(first) || value.len() == first.len_utf8() {
        return Err(dotenv_parse_error(path, line, "引号没有闭合"));
    }

    Ok(SourceValue {
        raw: value[first.len_utf8()..value.len() - first.len_utf8()].to_owned(),
        quoted: true,
    })
}

fn dotenv_parse_error(path: &str, line: usize, message: &str) -> Error {
    ErrorKind::DotenvParse {
        path: path.to_owned(),
        line,
        message: message.to_owned(),
    }
    .into()
}

fn valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };

    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|character| character == '_' || character.is_ascii_alphanumeric())
}

fn merge_env_values(object: &mut Map<String, Value>, values: BTreeMap<String, SourceValue>) {
    for (key, value) in values {
        object.insert(field_name_from_env_key(&key), value.inferred_json());
    }
}

fn field_name_from_env_key(key: &str) -> String {
    key.to_ascii_lowercase()
}

fn source_values_from_env(
    env_values: impl IntoIterator<Item = (String, String)>,
) -> BTreeMap<String, SourceValue> {
    env_values
        .into_iter()
        .map(|(key, value)| {
            (
                key,
                SourceValue {
                    raw: value,
                    quoted: false,
                },
            )
        })
        .collect()
}

fn real_env_values() -> Vec<(String, String)> {
    env::vars_os()
        .filter_map(|(key, value)| {
            let key = key.into_string().ok()?;
            let value = value.into_string().ok()?;
            Some((key, value))
        })
        .collect()
}

fn display_path(path: &StdPath) -> String {
    path.display().to_string()
}

#[cfg(test)]
mod tests {
    use std::{
        fs as std_fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use serde::Deserialize;

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct AppConfig {
        host: String,
        port: u16,
        debug: bool,
    }

    fn temp_root(test_name: &str) -> std::result::Result<PathBuf, Box<dyn StdError>> {
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let root = env::temp_dir().join(format!(
            "easy-rust-config-{}-{test_name}-{nanos}",
            std::process::id()
        ));
        std_fs::create_dir_all(&root)?;
        Ok(root)
    }

    #[test]
    fn get_reads_values_from_dotenv() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("get-dotenv")?;
        let dotenv = root.join(".env");
        std_fs::write(
            &dotenv,
            "NAME=Ada\nPORT=8000\nDEBUG=true\nexport MODE=local\n",
        )?;

        let name: String = get_from_sources("NAME", &dotenv, [])?.ok_or("missing NAME")?;
        let port: u16 = get_from_sources("PORT", &dotenv, [])?.ok_or("missing PORT")?;
        let debug: bool = get_from_sources("DEBUG", &dotenv, [])?.ok_or("missing DEBUG")?;
        let mode: String = get_from_sources("MODE", &dotenv, [])?.ok_or("missing MODE")?;

        assert_eq!(name, "Ada");
        assert_eq!(port, 8000);
        assert!(debug);
        assert_eq!(mode, "local");
        Ok(())
    }

    #[test]
    fn real_env_overrides_dotenv() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("env-overrides")?;
        let dotenv = root.join(".env");
        std_fs::write(&dotenv, "PORT=8000\n")?;

        let port: u16 =
            get_from_sources("PORT", &dotenv, [("PORT".to_owned(), "9000".to_owned())])?
                .ok_or("missing PORT")?;

        assert_eq!(port, 9000);
        Ok(())
    }

    #[test]
    fn get_uses_json_dotenv_env_precedence() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("get-precedence")?;
        let config = root.join("config.json");
        let dotenv = root.join(".env");
        std_fs::write(&config, r#"{"port":7000,"host":"file"}"#)?;
        std_fs::write(&dotenv, "PORT=8000\n")?;

        let port: u16 = get_from_sources_with_config(
            "PORT",
            Some(&config),
            &dotenv,
            [("PORT".to_owned(), "9000".to_owned())],
        )?
        .ok_or("missing PORT")?;
        let host: String = get_from_sources_with_config("HOST", Some(&config), &dotenv, [])?
            .ok_or("missing HOST")?;

        assert_eq!(port, 9000);
        assert_eq!(host, "file");
        Ok(())
    }

    #[test]
    fn get_or_returns_default_and_preserves_parse_errors()
    -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("get-or")?;
        let missing = root.join(".env");
        let broken = root.join("broken.env");
        std_fs::write(&broken, "PORT=abc\n")?;

        let port = get_or_from_sources("PORT", 8000_u16, &missing, [])?;
        assert_eq!(port, 8000);

        let error = match get_from_sources::<u16>("PORT", &broken, []) {
            Ok(_) => return Err("expected parse error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Parse { key, .. } => assert_eq!(key, "PORT"),
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[test]
    fn require_missing_returns_missing_error() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("missing")?;
        let dotenv = root.join(".env");

        let error = match require_from_sources::<String>("DATABASE_URL", &dotenv, []) {
            Ok(_) => return Err("expected missing value".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Missing { key } => assert_eq!(key, "DATABASE_URL"),
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[test]
    fn load_merges_json_dotenv_and_real_env() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("load")?;
        let config = root.join("config.json");
        let dotenv = root.join(".env");
        std_fs::write(&config, r#"{"host":"file","port":7000,"debug":false}"#)?;
        std_fs::write(&dotenv, "PORT=8000\n")?;

        let app: AppConfig = load_from_sources(
            "load",
            Some(&config),
            ConfigFormat::Json,
            true,
            &dotenv,
            [
                ("HOST".to_owned(), "env".to_owned()),
                ("DEBUG".to_owned(), "true".to_owned()),
            ],
        )?;

        assert_eq!(
            app,
            AppConfig {
                host: "env".to_owned(),
                port: 8000,
                debug: true,
            }
        );
        Ok(())
    }

    #[test]
    fn auto_allows_missing_config_json() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("auto-missing")?;
        let config = root.join("config.json");
        let dotenv = root.join(".env");
        std_fs::write(&dotenv, "HOST=dotenv\nPORT=8000\n")?;

        let app: AppConfig = load_from_sources(
            "auto",
            Some(&config),
            ConfigFormat::Json,
            false,
            &dotenv,
            [("DEBUG".to_owned(), "true".to_owned())],
        )?;

        assert_eq!(
            app,
            AppConfig {
                host: "dotenv".to_owned(),
                port: 8000,
                debug: true,
            }
        );
        Ok(())
    }

    #[test]
    fn missing_dotenv_is_ok_and_dotenv_parse_error_has_line()
    -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("dotenv-parse")?;
        let missing = root.join(".env");
        let broken = root.join("broken.env");
        std_fs::write(&broken, "GOOD=1\nBAD LINE\n")?;

        let value: Option<String> = get_from_sources("GOOD", &missing, [])?;
        assert_eq!(value, None);

        let error = match get_from_sources::<String>("GOOD", &broken, []) {
            Ok(_) => return Err("expected dotenv parse error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::DotenvParse { line, .. } => assert_eq!(*line, 2),
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[test]
    fn json_root_must_be_object() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("shape")?;
        let config = root.join("config.json");
        let dotenv = root.join(".env");
        std_fs::write(&config, "[1,2,3]")?;

        let error = match load_from_sources::<AppConfig>(
            "load",
            Some(&config),
            ConfigFormat::Json,
            true,
            &dotenv,
            [],
        ) {
            Ok(_) => return Err("expected shape error".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Shape { operation, .. } => assert_eq!(*operation, "load"),
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[test]
    fn load_toml_merges_dotenv_and_real_env() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("load-toml")?;
        let config = root.join("config.toml");
        let dotenv = root.join(".env");
        std_fs::write(&config, "host = 'file'\nport = 7000\ndebug = false\n")?;
        std_fs::write(&dotenv, "PORT=8000\n")?;

        let app: AppConfig = load_from_sources(
            "load_toml",
            Some(&config),
            ConfigFormat::Toml,
            true,
            &dotenv,
            [
                ("HOST".to_owned(), "env".to_owned()),
                ("DEBUG".to_owned(), "true".to_owned()),
            ],
        )?;

        assert_eq!(
            app,
            AppConfig {
                host: "env".to_owned(),
                port: 8000,
                debug: true,
            }
        );
        Ok(())
    }

    #[test]
    fn load_yaml_merges_dotenv_and_real_env() -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("load-yaml")?;
        let config = root.join("config.yaml");
        let dotenv = root.join(".env");
        std_fs::write(&config, "host: file\nport: 7000\ndebug: false\n")?;
        std_fs::write(&dotenv, "PORT=8000\n")?;

        let app: AppConfig = load_from_sources(
            "load_yaml",
            Some(&config),
            ConfigFormat::Yaml,
            true,
            &dotenv,
            [
                ("HOST".to_owned(), "env".to_owned()),
                ("DEBUG".to_owned(), "true".to_owned()),
            ],
        )?;

        assert_eq!(
            app,
            AppConfig {
                host: "env".to_owned(),
                port: 8000,
                debug: true,
            }
        );
        Ok(())
    }

    #[test]
    fn toml_and_yaml_errors_include_operation_and_path()
    -> std::result::Result<(), Box<dyn StdError>> {
        let root = temp_root("format-errors")?;
        let dotenv = root.join(".env");
        let bad_toml = root.join("bad.toml");
        let bad_yaml = root.join("bad.yaml");
        let yaml_list = root.join("list.yaml");
        let missing = root.join("missing.toml");
        std_fs::write(&bad_toml, "host = ")?;
        std_fs::write(&bad_yaml, "host: [")?;
        std_fs::write(&yaml_list, "- 1\n- 2\n")?;

        let toml_error = match load_from_sources::<AppConfig>(
            "load_toml",
            Some(&bad_toml),
            ConfigFormat::Toml,
            true,
            &dotenv,
            [],
        ) {
            Ok(_) => return Err("expected toml error".into()),
            Err(error) => error,
        };
        assert!(toml_error.to_string().contains("load_toml"));
        assert!(toml_error.to_string().contains("bad.toml"));
        assert!(matches!(toml_error.kind(), ErrorKind::TomlDecode { .. }));
        assert!(toml_error.source().is_some());

        let yaml_error = match load_from_sources::<AppConfig>(
            "load_yaml",
            Some(&bad_yaml),
            ConfigFormat::Yaml,
            true,
            &dotenv,
            [],
        ) {
            Ok(_) => return Err("expected yaml error".into()),
            Err(error) => error,
        };
        assert!(yaml_error.to_string().contains("load_yaml"));
        assert!(yaml_error.to_string().contains("bad.yaml"));
        assert!(matches!(yaml_error.kind(), ErrorKind::YamlDecode { .. }));
        assert!(yaml_error.source().is_some());

        let shape_error = match load_from_sources::<AppConfig>(
            "load_yaml",
            Some(&yaml_list),
            ConfigFormat::Yaml,
            true,
            &dotenv,
            [],
        ) {
            Ok(_) => return Err("expected yaml shape error".into()),
            Err(error) => error,
        };
        match shape_error.kind() {
            ErrorKind::Shape { operation, message } => {
                assert_eq!(*operation, "load_yaml");
                assert!(message.contains("YAML"));
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        let read_error = match load_from_sources::<AppConfig>(
            "load_toml",
            Some(&missing),
            ConfigFormat::Toml,
            true,
            &dotenv,
            [],
        ) {
            Ok(_) => return Err("expected missing file error".into()),
            Err(error) => error,
        };
        assert!(read_error.to_string().contains("missing.toml"));
        assert!(matches!(read_error.kind(), ErrorKind::Read { .. }));
        Ok(())
    }
}
