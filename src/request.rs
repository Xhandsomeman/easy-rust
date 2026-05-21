//! 极简 HTTP 请求 API。
//!
//! 默认入口尽量贴近 Python `requests`：`request::get(url).await?` 发送请求，
//! `request::post(url).json(&body).await?` 或 `.form(&data).await?` 发送请求体。
//! 返回的 [`Response`] 可以反复读取文本、字节或 JSON。连接池、重试和超时都有内部默认值。

use std::{
    collections::hash_map::DefaultHasher,
    error::Error as StdError,
    fmt, fs as std_fs,
    future::{Future, IntoFuture},
    hash::{Hash, Hasher},
    net::IpAddr,
    pin::Pin,
    sync::{Arc, OnceLock},
    time::{Duration, SystemTime},
};

use bytes::Bytes;
use encoding_rs::Encoding;
use http::{
    HeaderMap, HeaderName, HeaderValue, Method, StatusCode,
    header::{AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER, USER_AGENT},
};
use serde::{Serialize, de::DeserializeOwned};
use url_crate::Url;

use base64_crate::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};

use crate::fs::Path as FsPath;

const DEFAULT_USER_AGENT: &str = concat!("easy-rust/", env!("CARGO_PKG_VERSION"));
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_POOL_IDLE_TIMEOUT: Duration = Duration::from_secs(90);
const DEFAULT_POOL_MAX_IDLE_PER_HOST: usize = 32;
const DEFAULT_REDIRECT_LIMIT: usize = 10;
const ERROR_BODY_PREVIEW_LIMIT: usize = 4096;

static SHARED_CLIENT: OnceLock<std::result::Result<Client, String>> = OnceLock::new();

/// request 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。用户代码里常见写法是
/// `async fn main() -> request::Result<()>`，然后用 `?` 把错误继续向上传递。
pub type Result<T> = std::result::Result<T, Error>;

/// request 模块返回的轻量错误类型。
///
/// 这个类型本身很小，内部用 [`ErrorKind`] 保存具体错误细节。这样既保留详细错误信息，
/// 又不会让所有 `Result<T, Error>` 都携带一个很大的错误枚举。
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
    /// 当调用方需要区分 URL 错误、JSON 错误、状态码错误等情况时，使用这个方法匹配
    /// [`ErrorKind`]。
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

/// request 模块的具体错误原因。
///
/// 普通用户通常只需要用 `?` 传播 [`Error`]；需要做精细错误处理时，再匹配这个枚举。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// HTTP client 构建失败。
    #[error("request client_build failed")]
    ClientBuild,

    /// 全局共享默认 client 初始化失败。
    #[error("shared HTTP client is unavailable: {message}")]
    SharedClient {
        /// 初始化失败信息。
        message: String,
    },

    /// URL 解析失败。
    #[error("request invalid_url `{url}` failed")]
    InvalidUrl {
        /// 用户传入的原始 URL。
        url: String,
    },

    /// HTTP 方法名解析失败。
    #[error("request invalid_method `{method}` failed")]
    InvalidMethod {
        /// 用户传入的原始方法名。
        method: String,
    },

    /// 查询参数序列化失败。
    #[error("request params failed")]
    QuerySerialize,

    /// JSON 请求体序列化失败。
    #[error("request json failed")]
    JsonSerialize,

    /// 表单请求体序列化失败。
    #[error("request form failed")]
    FormSerialize,

    /// 上传文件读取失败。
    #[error("request upload_file `{path}` as `{field}` failed")]
    UploadRead {
        /// 表单字段名。
        field: String,
        /// 发生错误的文件路径。
        path: FsPath,
    },

    /// 下载响应体保存失败。
    #[error("request {operation} `{url}` to `{path}` failed")]
    Download {
        /// 发生错误的操作名，例如 `download`。
        operation: &'static str,
        /// 下载 URL。
        url: String,
        /// 保存路径。
        path: FsPath,
    },

    /// 响应体保存失败。
    #[error("request {operation} `{url}` to `{path}` failed")]
    Save {
        /// 发生错误的操作名，例如 `save`。
        operation: &'static str,
        /// 响应 URL。
        url: String,
        /// 保存路径。
        path: FsPath,
    },

    /// 认证请求头构造失败。
    #[error("request {operation} auth `{scheme}` failed")]
    Auth {
        /// 发生错误的操作名，例如 `bearer`。
        operation: &'static str,
        /// 认证方案，例如 `Bearer` 或 `Basic`。
        scheme: &'static str,
    },

    /// 请求头名称解析失败。
    #[error("request header `{name}` failed")]
    InvalidHeaderName {
        /// 用户传入的请求头名称。
        name: String,
    },

    /// 请求头值解析失败。
    #[error("request header_value `{name}` failed")]
    InvalidHeaderValue {
        /// 发生错误的请求头名称。
        name: String,
    },

    /// 网络传输层请求失败。
    ///
    /// 例如连接失败、DNS 失败、TLS 失败或请求超时。
    #[error("HTTP {method} {url} failed after {attempts} attempt(s)")]
    Transport {
        /// HTTP 方法。
        method: String,
        /// 请求 URL。
        url: String,
        /// 实际尝试次数。
        attempts: u32,
    },

    /// HTTP 状态码错误。
    ///
    /// 默认请求不会因为 404/500 自动返回这个错误；只有显式调用
    /// [`Response::raise_for_status`] 时才会返回。
    #[error("HTTP {method} {url} returned {status} after {attempts} attempt(s): {body_preview}")]
    Status {
        /// HTTP 方法。
        method: String,
        /// 请求 URL。
        url: String,
        /// 响应状态码。
        status: u16,
        /// 实际尝试次数。
        attempts: u32,
        /// 截断后的响应体预览，用于诊断错误。
        body_preview: String,
    },

    /// 响应体读取失败。
    #[error("request body_read `{url}` failed")]
    BodyRead {
        /// 响应 URL。
        url: String,
    },

    /// 响应体 JSON 解析失败。
    #[error("request json_decode `{url}` failed")]
    JsonDecode {
        /// 响应 URL。
        url: String,
    },

    /// 域名解析失败。
    #[error("request lookup_host `{host}` failed")]
    LookupHost {
        /// 调用方传入的主机名。
        host: String,
    },

    /// IP 地址解析失败。
    #[error("request parse_ip `{input}` failed")]
    InvalidIp {
        /// 调用方传入的 IP 文本。
        input: String,
    },
}

/// 瞬时失败重试策略。
///
/// 默认只重试幂等请求方法，例如 `GET`、`HEAD`、`OPTIONS`、`PUT`、`DELETE`。
/// 默认重试常见瞬时状态码，例如 `408`、`429`、`500`、`502`、`503`、`504`。
#[derive(Clone, Debug)]
struct RetryPolicy {
    max_retries: u32,
    initial_backoff: Duration,
    max_backoff: Duration,
    retry_non_idempotent: bool,
    retry_statuses: Vec<StatusCode>,
}

impl RetryPolicy {
    fn should_retry_method(&self, method: &Method) -> bool {
        self.retry_non_idempotent
            || matches!(
                *method,
                Method::GET | Method::HEAD | Method::OPTIONS | Method::PUT | Method::DELETE
            )
    }

    fn should_retry_status(&self, status: StatusCode) -> bool {
        self.retry_statuses.contains(&status)
    }
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_retries: 2,
            initial_backoff: Duration::from_millis(100),
            max_backoff: Duration::from_secs(2),
            retry_non_idempotent: false,
            retry_statuses: vec![
                StatusCode::REQUEST_TIMEOUT,
                StatusCode::TOO_MANY_REQUESTS,
                StatusCode::INTERNAL_SERVER_ERROR,
                StatusCode::BAD_GATEWAY,
                StatusCode::SERVICE_UNAVAILABLE,
                StatusCode::GATEWAY_TIMEOUT,
            ],
        }
    }
}

/// Python `requests` 风格的请求对象。
///
/// 这个对象只是描述请求，真正发送发生在 `.await?` 时。
///
/// ```ignore
/// let response = request::get(url).await?;
/// let response = request::post(url).json(&body).await?;
/// ```
#[derive(Debug)]
pub struct Request {
    method: PendingMethod,
    url: String,
    headers: Vec<PendingHeader>,
    query: Vec<String>,
    body: Body,
    timeout: Option<Duration>,
    pending_error: Option<Error>,
}

impl Request {
    fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method: PendingMethod::Typed(method),
            url: url.into(),
            headers: Vec::new(),
            query: Vec::new(),
            body: Body::Empty,
            timeout: None,
            pending_error: None,
        }
    }

    fn new_raw(method: impl ToString, url: impl Into<String>) -> Self {
        Self {
            method: PendingMethod::Raw(method.to_string()),
            url: url.into(),
            headers: Vec::new(),
            query: Vec::new(),
            body: Body::Empty,
            timeout: None,
            pending_error: None,
        }
    }

    /// 批量添加字符串请求头。
    ///
    /// 可以传数组、`Vec` 或其他 `(key, value)` 迭代器。后续发送时才会校验 header 名称和值是否合法。
    #[must_use]
    pub fn headers<K, V, I>(mut self, headers: I) -> Self
    where
        K: ToString,
        V: ToString,
        I: IntoIterator<Item = (K, V)>,
    {
        for (name, value) in headers {
            self.headers
                .push(PendingHeader::Raw(name.to_string(), value.to_string()));
        }
        self
    }

    /// 设置 Bearer token 认证。
    ///
    /// 这个方法会设置 `Authorization: Bearer ...`。token 包含非法 header 字符时，错误会保存在
    /// 请求里，并在 `.await?` 时返回。
    #[must_use]
    pub fn bearer(mut self, token: impl AsRef<str>) -> Self {
        self.set_auth("bearer", "Bearer", format!("Bearer {}", token.as_ref()));
        self
    }

    /// 设置 Basic 认证。
    ///
    /// 这个方法会把 `username:password` 做 Base64 编码，并设置 `Authorization: Basic ...`。
    #[must_use]
    pub fn basic(mut self, username: impl AsRef<str>, password: impl AsRef<str>) -> Self {
        let raw = format!("{}:{}", username.as_ref(), password.as_ref());
        self.set_auth(
            "basic",
            "Basic",
            format!("Basic {}", BASE64_STANDARD.encode(raw)),
        );
        self
    }

    /// 添加 URL 查询参数。
    ///
    /// 参数通过 serde 序列化为 query string。这个方法保持 Python `requests` 的感觉：
    /// 如果参数序列化失败，错误会保存在请求里，并在 `.await?` 时返回。
    #[must_use]
    pub fn params<T>(mut self, params: &T) -> Self
    where
        T: Serialize + ?Sized,
    {
        match serde_urlencoded::to_string(params) {
            Ok(encoded) if !encoded.is_empty() => self.query.push(encoded),
            Ok(_) => {}
            Err(source) => self.store_error(Error::with_source(ErrorKind::QuerySerialize, source)),
        }
        self
    }

    /// 设置 JSON 请求体，对齐 Python `requests` 的 `json=` 参数。
    ///
    /// 会自动把请求体序列化为 JSON，并设置 `Content-Type: application/json`。
    /// 如果序列化失败，错误会保存在请求里，并在 `.await?` 时返回。
    #[must_use]
    pub fn json<T>(mut self, body: &T) -> Self
    where
        T: Serialize + ?Sized,
    {
        if let Err(error) = self.set_json_body(body) {
            self.store_error(error);
        }
        self
    }

    /// 设置表单请求体。
    ///
    /// 会把数据序列化为 `application/x-www-form-urlencoded`，对齐 Python `requests` 的
    /// 普通表单提交主路径，不处理 multipart 文件上传。如果序列化失败，错误会保存在请求里，
    /// 并在 `.await?` 时返回。
    #[must_use]
    pub fn form<T>(mut self, body: &T) -> Self
    where
        T: Serialize + ?Sized,
    {
        if let Err(error) = self.set_form_body(body) {
            self.store_error(error);
        }
        self
    }

    /// 设置原始请求体，对齐 Python `requests` 的 `data=` 参数。
    ///
    /// 不会自动设置 `Content-Type`。如果需要指定类型，请配合 [`Request::headers`] 使用。
    #[must_use]
    pub fn data(mut self, body: impl AsRef<[u8]>) -> Self {
        self.body = Body::Bytes {
            bytes: Bytes::copy_from_slice(body.as_ref()),
            content_type: None,
        };
        self
    }

    /// 上传文件。
    ///
    /// 这个方法会把请求体切换为 `multipart/form-data`，字段名使用 `name`，文件名自动取路径文件名。
    /// 文件会在这里读入内存，保证请求重试时可以重新发送。
    #[must_use]
    pub fn upload_file(mut self, name: impl Into<String>, path: impl Into<FsPath>) -> Self {
        let name = name.into();
        let path = path.into();
        match std::fs::read(path.as_std_path()) {
            Ok(bytes) => {
                let file_name = path.name().unwrap_or_else(|| "file".to_owned());
                self.push_upload(UploadPart::Bytes {
                    name,
                    file_name,
                    bytes: Bytes::from(bytes),
                });
            }
            Err(source) => self.store_error(Error::with_source(
                ErrorKind::UploadRead { field: name, path },
                source,
            )),
        }
        self
    }

    /// 上传文本字段。
    ///
    /// 这个方法会把请求体切换为 `multipart/form-data`，适合和 [`upload_file`](Self::upload_file)
    /// 一起提交普通文本字段。
    #[must_use]
    pub fn upload_text(mut self, name: impl Into<String>, text: impl Into<String>) -> Self {
        self.push_upload(UploadPart::Text {
            name: name.into(),
            text: text.into(),
        });
        self
    }

    /// 上传内存中的字节内容。
    ///
    /// 需要显式传入文件名，方便服务端按普通文件字段接收。
    #[must_use]
    pub fn upload_bytes(
        mut self,
        name: impl Into<String>,
        file_name: impl Into<String>,
        bytes: impl AsRef<[u8]>,
    ) -> Self {
        self.push_upload(UploadPart::Bytes {
            name: name.into(),
            file_name: file_name.into(),
            bytes: Bytes::copy_from_slice(bytes.as_ref()),
        });
        self
    }

    /// 覆盖当前请求的总超时时间。
    ///
    /// 只影响这一条请求，不会修改全局默认 client。推荐配合 `time::seconds(5)` 这类
    /// 简单时间入口使用。
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    async fn send(self) -> Result<Response> {
        self.into_request_builder()?.send().await
    }

    fn into_request_builder(self) -> Result<RequestBuilder> {
        if let Some(error) = self.pending_error {
            return Err(error);
        }

        let method = self.method.parse()?;
        let mut request = Client::shared()?.request(method, &self.url)?;

        for header in self.headers {
            request = match header {
                PendingHeader::Raw(name, value) => request.header(name, value)?,
                PendingHeader::Parsed(name, value) => request.parsed_header(name, value),
            };
        }

        request.query = self.query;
        request.body = self.body;
        request.timeout = self.timeout;

        Ok(request)
    }

    fn set_json_body<T>(&mut self, body: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        let bytes = serde_json::to_vec(body)
            .map_err(|source| Error::with_source(ErrorKind::JsonSerialize, source))?;
        self.body = Body::Bytes {
            bytes: Bytes::from(bytes),
            content_type: Some(HeaderValue::from_static("application/json")),
        };
        Ok(())
    }

    fn set_form_body<T>(&mut self, body: &T) -> Result<()>
    where
        T: Serialize + ?Sized,
    {
        let encoded = serde_urlencoded::to_string(body)
            .map_err(|source| Error::with_source(ErrorKind::FormSerialize, source))?;
        self.body = Body::Bytes {
            bytes: Bytes::from(encoded),
            content_type: Some(HeaderValue::from_static(
                "application/x-www-form-urlencoded",
            )),
        };
        Ok(())
    }

    fn store_error(&mut self, error: Error) {
        if self.pending_error.is_none() {
            self.pending_error = Some(error);
        }
    }

    fn set_auth(&mut self, operation: &'static str, scheme: &'static str, value: String) {
        match HeaderValue::from_str(&value) {
            Ok(value) => self
                .headers
                .push(PendingHeader::Parsed(AUTHORIZATION, value)),
            Err(source) => self.store_error(Error::with_source(
                ErrorKind::Auth { operation, scheme },
                source,
            )),
        }
    }

    fn push_upload(&mut self, part: UploadPart) {
        match &mut self.body {
            Body::Multipart { parts } => parts.push(part),
            Body::Empty | Body::Bytes { .. } => {
                self.body = Body::Multipart { parts: vec![part] };
            }
        }
    }
}

impl IntoFuture for Request {
    type IntoFuture = Pin<Box<dyn Future<Output = Self::Output> + Send>>;
    type Output = Result<Response>;

    fn into_future(self) -> Self::IntoFuture {
        Box::pin(async move { self.send().await })
    }
}

/// Python `requests.Session` 风格的会话句柄。
///
/// 当前实现使用共享连接池，适合把请求入口命名为 `session` 后在多处复用。
#[derive(Clone, Copy, Debug, Default)]
pub struct Session;

impl Session {
    /// 创建一个使用共享连接池的 session。
    ///
    /// 这个方法不会立即发起网络请求。
    #[must_use]
    pub fn new() -> Self {
        Self
    }

    /// 创建一个 `GET` 请求。
    pub fn get(&self, url: impl Into<String>) -> Request {
        Request::new(Method::GET, url)
    }

    /// 创建一个 `HEAD` 请求。
    pub fn head(&self, url: impl Into<String>) -> Request {
        Request::new(Method::HEAD, url)
    }

    /// 创建一个 `OPTIONS` 请求。
    pub fn options(&self, url: impl Into<String>) -> Request {
        Request::new(Method::OPTIONS, url)
    }

    /// 创建一个 `POST` 请求。
    pub fn post(&self, url: impl Into<String>) -> Request {
        Request::new(Method::POST, url)
    }

    /// 创建一个 `PUT` 请求。
    pub fn put(&self, url: impl Into<String>) -> Request {
        Request::new(Method::PUT, url)
    }

    /// 创建一个 `PATCH` 请求。
    pub fn patch(&self, url: impl Into<String>) -> Request {
        Request::new(Method::PATCH, url)
    }

    /// 创建一个 `DELETE` 请求。
    pub fn delete(&self, url: impl Into<String>) -> Request {
        Request::new(Method::DELETE, url)
    }

    /// 创建一个自定义 HTTP 方法请求。
    pub fn request(&self, method: impl ToString, url: impl Into<String>) -> Request {
        Request::new_raw(method, url)
    }
}

/// 可复用 HTTP client。
///
/// 这是高级入口，用于需要自定义生产配置的场景。普通 Python 风格代码优先使用
/// [`get`]、[`post`] 或 [`Session`]。
#[derive(Clone, Debug)]
struct Client {
    inner: reqwest::Client,
    config: Arc<ClientConfig>,
}

impl Client {
    /// 使用生产默认值创建 client。
    ///
    /// 如果只是普通请求，更推荐使用 [`client`] 或 [`Session::new`]。
    fn new() -> Result<Self> {
        Self::from_config(ClientConfig::default())
    }

    /// 返回进程级共享 client。
    ///
    /// 共享 client 由零配置入口内部使用，具备连接池复用能力。
    fn shared() -> Result<&'static Self> {
        match SHARED_CLIENT.get_or_init(|| Self::new().map_err(|error| error.to_string())) {
            Ok(client) => Ok(client),
            Err(message) => Err(ErrorKind::SharedClient {
                message: message.clone(),
            }
            .into()),
        }
    }

    /// 使用自定义 HTTP 方法创建请求。
    ///
    /// 这里接收已经解析好的 [`Method`]，所以适合高级代码。
    fn request(&self, method: Method, url: impl AsRef<str>) -> Result<RequestBuilder> {
        let raw_url = url.as_ref();
        let url = Url::parse(raw_url).map_err(|source| {
            Error::with_source(
                ErrorKind::InvalidUrl {
                    url: raw_url.to_owned(),
                },
                source,
            )
        })?;

        Ok(RequestBuilder {
            client: self.clone(),
            method,
            url,
            headers: HeaderMap::new(),
            query: Vec::new(),
            body: Body::Empty,
            timeout: None,
        })
    }

    fn from_config(config: ClientConfig) -> Result<Self> {
        let mut headers = config.default_headers.clone();
        if !headers.contains_key(USER_AGENT) {
            headers.insert(
                USER_AGENT,
                HeaderValue::from_str(&config.user_agent).map_err(|source| {
                    Error::with_source(
                        ErrorKind::InvalidHeaderValue {
                            name: USER_AGENT.as_str().to_owned(),
                        },
                        source,
                    )
                })?,
            );
        }

        let inner = reqwest::Client::builder()
            .default_headers(headers)
            .timeout(config.timeout)
            .connect_timeout(config.connect_timeout)
            .pool_idle_timeout(config.pool_idle_timeout)
            .pool_max_idle_per_host(config.pool_max_idle_per_host)
            .redirect(reqwest::redirect::Policy::limited(config.redirect_limit))
            .https_only(config.https_only)
            .tcp_nodelay(true)
            .build()
            .map_err(|source| Error::with_source(ErrorKind::ClientBuild, source))?;

        Ok(Self {
            inner,
            config: Arc::new(config),
        })
    }
}

/// 单个 HTTP 请求的高级构建器。
///
/// 这是 `Client` 返回的底层请求构建器。普通代码优先使用 Python 风格的 [`Request`]。
#[derive(Clone, Debug)]
struct RequestBuilder {
    client: Client,
    method: Method,
    url: Url,
    headers: HeaderMap,
    query: Vec<String>,
    body: Body,
    timeout: Option<Duration>,
}

impl RequestBuilder {
    fn header(mut self, name: impl AsRef<str>, value: impl AsRef<str>) -> Result<Self> {
        let name = parse_header_name(name.as_ref())?;
        let value = parse_header_value(name.as_str(), value.as_ref())?;
        self.headers.insert(name, value);
        Ok(self)
    }

    fn parsed_header(mut self, name: HeaderName, value: HeaderValue) -> Self {
        self.headers.insert(name, value);
        self
    }

    async fn send(self) -> Result<Response> {
        self.execute().await
    }

    async fn execute(self) -> Result<Response> {
        let policy = self.client.config.retry_policy.clone();
        let can_retry_method = policy.should_retry_method(&self.method);
        let max_attempts = policy.max_retries.saturating_add(1);
        let mut attempts = 0;

        loop {
            attempts += 1;
            let request_url = self.url_with_query();
            let result = self.build_reqwest_request(request_url.clone()).send().await;

            match result {
                Ok(response) => {
                    let status = response.status();
                    if can_retry_method
                        && attempts < max_attempts
                        && policy.should_retry_status(status)
                    {
                        let delay = retry_delay(
                            &policy,
                            attempts,
                            response.headers(),
                            &self.method,
                            &request_url,
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }

                    let response =
                        Response::from_reqwest(self.method.clone(), attempts, response).await?;
                    return Ok(response);
                }
                Err(source) => {
                    if can_retry_method && attempts < max_attempts && source.is_timeout() {
                        let delay = retry_delay(
                            &policy,
                            attempts,
                            &HeaderMap::new(),
                            &self.method,
                            &request_url,
                        );
                        tokio::time::sleep(delay).await;
                        continue;
                    }

                    return Err(Error::with_source(
                        ErrorKind::Transport {
                            method: self.method.as_str().to_owned(),
                            url: request_url.to_string(),
                            attempts,
                        },
                        source,
                    ));
                }
            }
        }
    }

    fn build_reqwest_request(&self, url: Url) -> reqwest::RequestBuilder {
        let mut request = self.client.inner.request(self.method.clone(), url);

        if !self.headers.is_empty() {
            request = request.headers(self.headers.clone());
        }

        if let Some(timeout) = self.timeout {
            request = request.timeout(timeout);
        }

        match &self.body {
            Body::Empty => request,
            Body::Bytes {
                bytes,
                content_type,
            } => {
                if let Some(content_type) = content_type {
                    request = request.header(CONTENT_TYPE, content_type.clone());
                }
                request.body(bytes.clone())
            }
            Body::Multipart { parts } => {
                let mut form = reqwest::multipart::Form::new();
                for part in parts {
                    match part {
                        UploadPart::Text { name, text } => {
                            form = form.text(name.clone(), text.clone());
                        }
                        UploadPart::Bytes {
                            name,
                            file_name,
                            bytes,
                        } => {
                            let part = reqwest::multipart::Part::bytes(bytes.to_vec())
                                .file_name(file_name.clone());
                            form = form.part(name.clone(), part);
                        }
                    }
                }
                request.multipart(form)
            }
        }
    }

    fn url_with_query(&self) -> Url {
        let mut url = self.url.clone();
        if self.query.is_empty() {
            return url;
        }

        let mut query = url.query().map_or_else(String::new, ToOwned::to_owned);
        for encoded in &self.query {
            if !query.is_empty() {
                query.push('&');
            }
            query.push_str(encoded);
        }
        url.set_query(Some(&query));
        url
    }
}

/// HTTP 响应。
///
/// 响应体在请求完成时已经缓冲到内存中，所以 [`Response::text`]、[`Response::content`]、
/// [`Response::json`] 可以重复调用，不会出现“响应体只能读一次”的问题。
#[derive(Debug)]
pub struct Response {
    method: Method,
    url: Url,
    status: StatusCode,
    headers: HeaderMap,
    body: Bytes,
    attempts: u32,
}

impl Response {
    /// 返回 Python `requests` 风格的数字状态码。
    ///
    /// 例如 `200`、`404`、`500`。
    #[must_use]
    pub fn status_code(&self) -> u16 {
        self.status.as_u16()
    }

    /// 判断响应是否成功。
    ///
    /// 与 Python `requests.Response.ok` 对齐：状态码小于 400 返回 `true`。
    #[must_use]
    pub fn ok(&self) -> bool {
        self.status.as_u16() < 400
    }

    /// 返回最终响应 URL。
    ///
    /// 如果请求发生重定向，这里返回重定向后的最终 URL。
    #[must_use]
    pub fn url(&self) -> &str {
        self.url.as_str()
    }

    /// 返回响应头列表。
    ///
    /// header 名称和值都会转换为普通字符串，避免把底层 HTTP header 类型暴露给调用方。
    #[must_use]
    pub fn headers(&self) -> Vec<(String, String)> {
        self.headers
            .iter()
            .map(|(name, value)| (name.as_str().to_owned(), header_value_to_string(value)))
            .collect()
    }

    /// 按名称读取单个响应头。
    ///
    /// 名称大小写不敏感；不存在或名称非法时返回 `None`。常见写法是
    /// `response.header("content-type")`。
    #[must_use]
    pub fn header(&self, name: impl AsRef<str>) -> Option<String> {
        let name = HeaderName::from_bytes(name.as_ref().as_bytes()).ok()?;
        self.headers.get(name).map(header_value_to_string)
    }

    /// 返回标准状态码原因短语。
    ///
    /// 例如 `200` 返回 `Some("OK")`。未知状态码可能返回 `None`。
    #[must_use]
    pub fn reason(&self) -> Option<&'static str> {
        self.status.canonical_reason()
    }

    /// 返回响应体字节。
    ///
    /// 这个方法不复制数据，只返回内部缓冲区引用。需要下载二进制内容或自己处理编码时使用它。
    #[must_use]
    pub fn content(&self) -> &[u8] {
        &self.body
    }

    /// 把响应体解码为文本。
    ///
    /// 会优先读取 `Content-Type` 中的 `charset`，没有 charset 时使用 UTF-8 lossy 解码。
    #[must_use]
    pub fn text(&self) -> String {
        decode_body_text(&self.headers, &self.body)
    }

    /// 把响应体解析为指定 JSON 类型。
    ///
    /// 类型 `T` 需要实现 serde 反序列化。解析失败会返回 [`ErrorKind::JsonDecode`]。
    pub fn json<T>(&self) -> Result<T>
    where
        T: DeserializeOwned,
    {
        serde_json::from_slice(&self.body).map_err(|source| {
            Error::with_source(
                ErrorKind::JsonDecode {
                    url: self.url.to_string(),
                },
                source,
            )
        })
    }

    /// 把响应体解析为动态 JSON 值。
    ///
    /// 适合暂时不想定义结构体、直接访问 JSON 字段的场景。需要强类型结果时优先使用
    /// [`Response::json`]。
    pub fn json_value(&self) -> Result<crate::json::Value> {
        self.json()
    }

    /// 如果状态码是 4xx 或 5xx，则返回状态码错误。
    ///
    /// 默认请求行为与 Python `requests` 一致：404/500 也会正常返回 [`Response`]。
    /// 调用这个方法后，状态码错误会变成 [`ErrorKind::Status`]，其中包含 method、url、status、
    /// attempts 和响应体预览。
    pub fn raise_for_status(&self) -> Result<&Self> {
        if self.ok() {
            return Ok(self);
        }

        Err(ErrorKind::Status {
            method: self.method.as_str().to_owned(),
            url: self.url.to_string(),
            status: self.status.as_u16(),
            attempts: self.attempts,
            body_preview: truncate_preview(&self.text()),
        }
        .into())
    }

    /// 把响应体保存到文件。
    ///
    /// 这个方法只负责保存已经拿到的响应体，不会检查状态码。需要把 404/500 转成错误时，
    /// 先调用 [`Response::raise_for_status`]。
    pub fn save(&self, path: impl Into<FsPath>) -> Result<FsPath> {
        let path = path.into();
        save_bytes_to_path("save", self.url.as_str(), &path, &self.body)?;
        Ok(path)
    }

    async fn from_reqwest(
        method: Method,
        attempts: u32,
        response: reqwest::Response,
    ) -> Result<Self> {
        let url = response.url().clone();
        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes().await.map_err(|source| {
            Error::with_source(
                ErrorKind::BodyRead {
                    url: url.to_string(),
                },
                source,
            )
        })?;

        Ok(Self {
            method,
            url,
            status,
            headers,
            body,
            attempts,
        })
    }
}

/// 创建一个 `GET` 请求。
///
/// 返回的 [`Request`] 可以直接 `.await?` 发送。
pub fn get(url: impl Into<String>) -> Request {
    Request::new(Method::GET, url)
}

/// 创建一个 `HEAD` 请求。
///
/// 通常用于只获取响应头。
pub fn head(url: impl Into<String>) -> Request {
    Request::new(Method::HEAD, url)
}

/// 创建一个 `OPTIONS` 请求。
///
/// 通常用于探测服务端支持的方法或跨域能力。
pub fn options(url: impl Into<String>) -> Request {
    Request::new(Method::OPTIONS, url)
}

/// 创建一个 `POST` 请求。
///
/// 常见写法是 `request::post(url).json(&body).await?`。
pub fn post(url: impl Into<String>) -> Request {
    Request::new(Method::POST, url)
}

/// 创建一个 `PUT` 请求。
///
/// 通常用于整体替换资源。
pub fn put(url: impl Into<String>) -> Request {
    Request::new(Method::PUT, url)
}

/// 创建一个 `PATCH` 请求。
///
/// 通常用于部分更新资源。
pub fn patch(url: impl Into<String>) -> Request {
    Request::new(Method::PATCH, url)
}

/// 创建一个 `DELETE` 请求。
///
/// 通常用于删除资源。
pub fn delete(url: impl Into<String>) -> Request {
    Request::new(Method::DELETE, url)
}

/// 使用自定义 HTTP 方法创建请求。
///
/// 适合非标准或扩展 HTTP 方法。
pub fn request(method: impl ToString, url: impl Into<String>) -> Request {
    Request::new_raw(method, url)
}

/// 下载 URL 内容到文件。
///
/// 这个函数发送 GET 请求，显式检查状态码，并把响应体保存到 `path`。写入会自动创建父目录。
pub async fn download(url: impl AsRef<str>, path: impl Into<FsPath>) -> Result<FsPath> {
    let url = url.as_ref().to_owned();
    let path = path.into();
    let response = get(&url).await.map_err(|source| {
        Error::with_source(
            ErrorKind::Download {
                operation: "download",
                url: url.clone(),
                path: path.clone(),
            },
            source,
        )
    })?;
    response.raise_for_status().map_err(|source| {
        Error::with_source(
            ErrorKind::Download {
                operation: "download",
                url: url.clone(),
                path: path.clone(),
            },
            source,
        )
    })?;
    response.save(&path).map_err(|source| {
        Error::with_source(
            ErrorKind::Download {
                operation: "download",
                url: url.clone(),
                path: path.clone(),
            },
            source,
        )
    })
}

/// 解析主机名对应的 IP 地址。
///
/// 返回字符串形式的 IP 列表，结果会去重并排序。这个函数只做 DNS 查询，不创建网络连接。
pub async fn lookup_host(host: impl AsRef<str>) -> Result<Vec<String>> {
    let host = host.as_ref();
    let addrs = tokio::net::lookup_host((host, 0)).await.map_err(|source| {
        Error::with_source(
            ErrorKind::LookupHost {
                host: host.to_owned(),
            },
            source,
        )
    })?;
    let mut output = Vec::new();
    for addr in addrs {
        let ip = addr.ip().to_string();
        if !output.contains(&ip) {
            output.push(ip);
        }
    }
    output.sort();
    Ok(output)
}

/// 解析 IP 地址并返回规范化字符串。
///
/// IPv4 和 IPv6 都支持；非法输入返回 [`ErrorKind::InvalidIp`]。
pub fn parse_ip(input: impl AsRef<str>) -> Result<String> {
    let input = input.as_ref();
    input
        .parse::<IpAddr>()
        .map(|ip| ip.to_string())
        .map_err(|source| {
            Error::with_source(
                ErrorKind::InvalidIp {
                    input: input.to_owned(),
                },
                source,
            )
        })
}

/// 判断文本是否是合法 IP 地址。
#[must_use]
pub fn is_ip(input: impl AsRef<str>) -> bool {
    input.as_ref().parse::<IpAddr>().is_ok()
}

#[derive(Clone, Debug)]
struct ClientConfig {
    timeout: Duration,
    connect_timeout: Duration,
    pool_idle_timeout: Duration,
    pool_max_idle_per_host: usize,
    redirect_limit: usize,
    user_agent: String,
    default_headers: HeaderMap,
    retry_policy: RetryPolicy,
    https_only: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            connect_timeout: DEFAULT_CONNECT_TIMEOUT,
            pool_idle_timeout: DEFAULT_POOL_IDLE_TIMEOUT,
            pool_max_idle_per_host: DEFAULT_POOL_MAX_IDLE_PER_HOST,
            redirect_limit: DEFAULT_REDIRECT_LIMIT,
            user_agent: DEFAULT_USER_AGENT.to_owned(),
            default_headers: HeaderMap::new(),
            retry_policy: RetryPolicy::default(),
            https_only: false,
        }
    }
}

#[derive(Clone, Debug)]
enum Body {
    Empty,
    Bytes {
        bytes: Bytes,
        content_type: Option<HeaderValue>,
    },
    Multipart {
        parts: Vec<UploadPart>,
    },
}

#[derive(Clone, Debug)]
enum UploadPart {
    Text {
        name: String,
        text: String,
    },
    Bytes {
        name: String,
        file_name: String,
        bytes: Bytes,
    },
}

#[derive(Clone, Debug)]
enum PendingMethod {
    Typed(Method),
    Raw(String),
}

impl PendingMethod {
    fn parse(self) -> Result<Method> {
        match self {
            Self::Typed(method) => Ok(method),
            Self::Raw(method) => Method::from_bytes(method.as_bytes()).map_err(|source| {
                Error::with_source(
                    ErrorKind::InvalidMethod {
                        method: method.clone(),
                    },
                    source,
                )
            }),
        }
    }
}

#[derive(Clone, Debug)]
enum PendingHeader {
    Raw(String, String),
    Parsed(HeaderName, HeaderValue),
}

fn save_bytes_to_path(
    operation: &'static str,
    url: &str,
    path: &FsPath,
    bytes: &[u8],
) -> Result<()> {
    if let Some(parent) = path.as_std_path().parent()
        && !parent.as_os_str().is_empty()
    {
        std_fs::create_dir_all(parent).map_err(|source| {
            Error::with_source(
                ErrorKind::Save {
                    operation,
                    url: url.to_owned(),
                    path: path.clone(),
                },
                source,
            )
        })?;
    }

    std_fs::write(path.as_std_path(), bytes).map_err(|source| {
        Error::with_source(
            ErrorKind::Save {
                operation,
                url: url.to_owned(),
                path: path.clone(),
            },
            source,
        )
    })
}

fn parse_header_name(name: &str) -> Result<HeaderName> {
    HeaderName::from_bytes(name.as_bytes()).map_err(|source| {
        Error::with_source(
            ErrorKind::InvalidHeaderName {
                name: name.to_owned(),
            },
            source,
        )
    })
}

fn parse_header_value(name: &str, value: &str) -> Result<HeaderValue> {
    HeaderValue::from_str(value).map_err(|source| {
        Error::with_source(
            ErrorKind::InvalidHeaderValue {
                name: name.to_owned(),
            },
            source,
        )
    })
}

fn retry_delay(
    policy: &RetryPolicy,
    attempt: u32,
    headers: &HeaderMap,
    method: &Method,
    url: &Url,
) -> Duration {
    if let Some(delay) = retry_after(headers) {
        return delay.min(policy.max_backoff);
    }

    let exponent = attempt.saturating_sub(1).min(16);
    let factor = 1_u32 << exponent;
    let base = policy
        .initial_backoff
        .saturating_mul(factor)
        .min(policy.max_backoff);

    stable_jitter(base, attempt, method, url)
}

fn retry_after(headers: &HeaderMap) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?;
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }

    let retry_at = httpdate::parse_http_date(value).ok()?;
    retry_at.duration_since(SystemTime::now()).ok()
}

fn header_value_to_string(value: &HeaderValue) -> String {
    String::from_utf8_lossy(value.as_bytes()).into_owned()
}

fn stable_jitter(base: Duration, attempt: u32, method: &Method, url: &Url) -> Duration {
    if base.is_zero() {
        return base;
    }

    let mut hasher = DefaultHasher::new();
    attempt.hash(&mut hasher);
    method.as_str().hash(&mut hasher);
    url.as_str().hash(&mut hasher);
    let percent = 80 + hasher.finish() % 41;
    base.mul_f64(percent as f64 / 100.0)
}

fn decode_body_text(headers: &HeaderMap, body: &Bytes) -> String {
    if let Some(encoding) = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(charset_from_content_type)
        .and_then(|charset| Encoding::for_label(charset.as_bytes()))
    {
        let (text, _, _) = encoding.decode(body);
        return text.into_owned();
    }

    String::from_utf8_lossy(body).into_owned()
}

fn charset_from_content_type(content_type: &str) -> Option<&str> {
    for part in content_type.split(';').skip(1) {
        if let Some((name, value)) = part.trim().split_once('=')
            && name.trim().eq_ignore_ascii_case("charset")
        {
            return Some(value.trim().trim_matches('"'));
        }
    }
    None
}

fn truncate_preview(body: &str) -> String {
    let mut preview = String::new();
    for ch in body.chars().take(ERROR_BODY_PREVIEW_LIMIT) {
        preview.push(ch);
    }

    if preview.len() < body.len() {
        preview.push_str("...");
    }

    preview
}

#[cfg(test)]
mod tests {
    use serde::{Deserialize, Serialize};
    use serde_json::json;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{
            body_json, body_string, body_string_contains, header, header_regex, method, path,
            query_param,
        },
    };

    use super::*;

    #[derive(Debug, Deserialize, PartialEq)]
    struct User {
        id: u64,
        name: String,
    }

    #[derive(Debug, Serialize)]
    struct CreateUser<'a> {
        name: &'a str,
    }

    #[tokio::test]
    async fn python_style_get_returns_response_with_json()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/users/3"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": 3,
                "name": "Lin"
            })))
            .mount(&server)
            .await;

        let response = super::get(format!("{}/users/3", server.uri())).await?;
        let user: User = response.json()?;
        let value = response.json_value()?;

        assert_eq!(
            user,
            User {
                id: 3,
                name: "Lin".to_owned()
            }
        );
        assert_eq!(value["name"], "Lin");
        Ok(())
    }

    #[tokio::test]
    async fn python_style_get_text_is_simple() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let text = super::get(format!("{}/health", server.uri()))
            .timeout(crate::time::seconds(5))
            .await?
            .text();

        assert_eq!(text, "ok");
        Ok(())
    }

    #[tokio::test]
    async fn python_style_post_json_sets_body()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/users"))
            .and(header("content-type", "application/json"))
            .and(body_json(json!({ "name": "Grace" })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "id": 2,
                "name": "Grace"
            })))
            .mount(&server)
            .await;

        let user: User = super::post(format!("{}/users", server.uri()))
            .json(&CreateUser { name: "Grace" })
            .await?
            .json()?;

        assert_eq!(
            user,
            User {
                id: 2,
                name: "Grace".to_owned()
            }
        );
        Ok(())
    }

    #[tokio::test]
    async fn python_style_post_form_sets_body()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/login"))
            .and(header("content-type", "application/x-www-form-urlencoded"))
            .and(body_string("name=Ada+Lovelace&path=%2Fhome"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let response = super::post(format!("{}/login", server.uri()))
            .form(&[("name", "Ada Lovelace"), ("path", "/home")])
            .await?;

        assert_eq!(response.text(), "ok");
        Ok(())
    }

    #[tokio::test]
    async fn upload_methods_send_multipart_body()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        let file = std::env::temp_dir().join(format!(
            "easy-rust-request-upload-{}.txt",
            std::process::id()
        ));
        std::fs::write(&file, "file body")?;

        Mock::given(method("POST"))
            .and(path("/upload"))
            .and(header_regex("content-type", "multipart/form-data"))
            .and(body_string_contains("name=\"note\""))
            .and(body_string_contains("hello"))
            .and(body_string_contains("name=\"file\""))
            .and(body_string_contains("filename=\""))
            .and(body_string_contains("file body"))
            .and(body_string_contains("name=\"blob\""))
            .and(body_string_contains("filename=\"blob.bin\""))
            .and(body_string_contains("abc"))
            .respond_with(ResponseTemplate::new(200).set_body_string("uploaded"))
            .mount(&server)
            .await;

        let response = post(format!("{}/upload", server.uri()))
            .upload_text("note", "hello")
            .upload_file("file", file.display().to_string())
            .upload_bytes("blob", "blob.bin", b"abc")
            .await?;

        assert_eq!(response.text(), "uploaded");
        let _ = std::fs::remove_file(file);
        Ok(())
    }

    #[tokio::test]
    async fn lookup_host_and_ip_helpers_work() -> std::result::Result<(), Box<dyn std::error::Error>>
    {
        let ips = lookup_host("localhost").await?;

        assert!(!ips.is_empty());
        assert_eq!(parse_ip("127.0.0.1")?, "127.0.0.1");
        assert!(is_ip("::1"));
        assert!(!is_ip("not an ip"));
        Ok(())
    }

    #[tokio::test]
    async fn python_style_params_and_headers_are_serialized()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/search"))
            .and(query_param("q", "rust"))
            .and(header("x-api-key", "secret"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let response = super::get(format!("{}/search", server.uri()))
            .params(&[("q", "rust")])
            .headers([("x-api-key", "secret")])
            .await?;

        assert_eq!(response.text(), "ok");
        Ok(())
    }

    #[tokio::test]
    async fn auth_helpers_set_authorization_header()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/bearer"))
            .and(header("authorization", "Bearer token123"))
            .respond_with(ResponseTemplate::new(200).set_body_string("bearer"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/basic"))
            .and(header("authorization", "Basic dXNlcjpwYXNz"))
            .respond_with(ResponseTemplate::new(200).set_body_string("basic"))
            .mount(&server)
            .await;

        let bearer = get(format!("{}/bearer", server.uri()))
            .bearer("token123")
            .await?;
        let basic = get(format!("{}/basic", server.uri()))
            .basic("user", "pass")
            .await?;

        assert_eq!(bearer.text(), "bearer");
        assert_eq!(basic.text(), "basic");

        let error = match get(format!("{}/bearer", server.uri()))
            .bearer("bad\nvalue")
            .await
        {
            Ok(response) => return Err(format!("expected auth error, got {response:?}").into()),
            Err(error) => error,
        };
        match error.kind() {
            ErrorKind::Auth { operation, scheme } => {
                assert_eq!(*operation, "bearer");
                assert_eq!(*scheme, "Bearer");
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }
        Ok(())
    }

    #[tokio::test]
    async fn download_and_response_save_write_body_to_file()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/file"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![0, 1, 2, 3]))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404).set_body_string("missing"))
            .mount(&server)
            .await;

        let root =
            std::env::temp_dir().join(format!("easy-rust-request-download-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let downloaded = root.join("nested/file.bin");
        let saved = root.join("saved/file.bin");

        let path = download(
            format!("{}/file", server.uri()),
            downloaded.display().to_string(),
        )
        .await?;
        assert_eq!(std::fs::read(path.as_std_path())?, vec![0, 1, 2, 3]);

        let response = get(format!("{}/file", server.uri())).await?;
        let saved_path = response.save(saved.display().to_string())?;
        assert_eq!(std::fs::read(saved_path.as_std_path())?, vec![0, 1, 2, 3]);

        let error = match download(
            format!("{}/missing", server.uri()),
            root.join("missing.bin").display().to_string(),
        )
        .await
        {
            Ok(path) => return Err(format!("expected download error, got {path}").into()),
            Err(error) => error,
        };
        assert!(error.to_string().contains("download"));
        assert!(error.to_string().contains("404"));

        let _ = std::fs::remove_dir_all(root);
        Ok(())
    }

    #[tokio::test]
    async fn session_get_needs_no_builder() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/health"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let session = Session::new();
        let response = session.get(format!("{}/health", server.uri())).await?;

        assert_eq!(response.text(), "ok");
        Ok(())
    }

    #[tokio::test]
    async fn session_handle_can_be_reused_for_multiple_requests()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/first"))
            .respond_with(ResponseTemplate::new(200).set_body_string("one"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/second"))
            .respond_with(ResponseTemplate::new(200).set_body_string("two"))
            .mount(&server)
            .await;

        let session = Session::new();
        let first = session.get(format!("{}/first", server.uri())).await?;
        let second = session.get(format!("{}/second", server.uri())).await?;

        assert_eq!(first.text(), "one");
        assert_eq!(second.text(), "two");
        Ok(())
    }

    #[tokio::test]
    async fn status_error_is_explicit_with_raise_for_status()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not here"))
            .mount(&server)
            .await;

        let response = super::get(format!("{}/missing", server.uri())).await?;

        assert_eq!(response.status_code(), 404);
        assert!(!response.ok());

        let error = match response.raise_for_status() {
            Ok(_) => return Err("expected raise_for_status to fail".into()),
            Err(error) => error,
        };

        match error.kind() {
            ErrorKind::Status {
                method,
                url,
                status,
                body_preview,
                attempts,
            } => {
                assert_eq!(method, "GET");
                assert!(url.ends_with("/missing"));
                assert_eq!(*status, 404);
                assert_eq!(*attempts, 1);
                assert!(body_preview.contains("not here"));
            }
            other => return Err(format!("unexpected error: {other}").into()),
        }

        Ok(())
    }

    #[tokio::test]
    async fn response_accessors_return_high_level_values()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/headers"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("x-request-id", "abc")
                    .set_body_string("body"),
            )
            .mount(&server)
            .await;

        let response = super::get(format!("{}/headers", server.uri())).await?;

        assert!(response.url().ends_with("/headers"));
        assert_eq!(response.header("x-request-id").as_deref(), Some("abc"));
        assert!(
            response
                .headers()
                .contains(&("x-request-id".to_owned(), "abc".to_owned()))
        );
        assert_eq!(response.content(), b"body");
        Ok(())
    }

    #[tokio::test]
    async fn retries_idempotent_transient_status()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/unstable"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/unstable"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ready"))
            .mount(&server)
            .await;

        let text = super::get(format!("{}/unstable", server.uri()))
            .await?
            .text();

        assert_eq!(text, "ready");
        Ok(())
    }
}
