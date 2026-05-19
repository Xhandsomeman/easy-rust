//! 极简爬虫 API。
//!
//! 这个模块只做单页抓取、HTML 解析和内容提取。普通用法是
//! `crawler::get(url).await?` 获取页面，然后用 `page.title()`、`page.links()`、
//! `page.select_text(".item")?` 提取内容。

use std::{error::Error as StdError, fmt};

use scraper_crate::{ElementRef, Html, Selector};
use url_crate::Url as BaseUrl;

use crate::request;

/// crawler 模块统一使用的结果类型。
///
/// 成功时返回 `T`，失败时返回 [`Error`]。常见写法是
/// `let page = crawler::get("https://example.com").await?;`。
pub type Result<T> = std::result::Result<T, Error>;

/// crawler 模块返回的轻量错误类型。
///
/// 具体错误原因保存在 [`ErrorKind`] 中。HTTP、URL 和底层解析错误会保留在
/// [`std::error::Error::source`] 里，公开错误信息只展示高层操作和关键上下文。
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
    /// 调用方需要区分抓取、基础 URL 或选择器错误时，可以匹配 [`ErrorKind`]。
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

/// crawler 模块的具体错误原因。
///
/// 错误信息会包含操作名和关键上下文，例如 URL 或 CSS selector。
#[derive(Debug, thiserror::Error)]
pub enum ErrorKind {
    /// 页面抓取失败。
    #[error("crawler {operation} `{url}` failed")]
    Fetch {
        /// 发生错误的操作名，例如 `get`。
        operation: &'static str,
        /// 用户请求的 URL。
        url: String,
    },

    /// 基础 URL 解析失败。
    #[error("crawler {operation} base url `{url}` failed")]
    Url {
        /// 发生错误的操作名，例如 `from_html_with_url`。
        operation: &'static str,
        /// 用户传入的基础 URL。
        url: String,
    },

    /// CSS 选择器解析失败。
    #[error("crawler {operation} selector `{selector}` failed")]
    Selector {
        /// 发生错误的操作名，例如 `select`。
        operation: &'static str,
        /// 用户传入的 CSS selector。
        selector: String,
    },
}

/// 高层 HTML 页面。
///
/// `Page` 保存原始 HTML 和内部解析结果，提供标题、全文、链接、图片和 CSS selector
/// 查询等常用读取方法。它不会暴露底层 DOM 或 parser 类型。
pub struct Page {
    html: String,
    document: Html,
    url: Option<String>,
    base_url: Option<BaseUrl>,
}

impl Page {
    /// 返回页面 URL。
    ///
    /// 通过 [`get`] 或 [`from_html_with_url`] 创建的页面会返回 `Some`；通过
    /// [`from_html`] 创建的页面没有基础 URL，返回 `None`。
    #[must_use]
    pub fn url(&self) -> Option<&str> {
        self.url.as_deref()
    }

    /// 返回原始 HTML。
    #[must_use]
    pub fn html(&self) -> &str {
        &self.html
    }

    /// 返回页面标题。
    ///
    /// 没有 `<title>` 时返回 `None`。标题文本会清理连续空白。
    #[must_use]
    pub fn title(&self) -> Option<String> {
        self.select_text("title")
            .ok()
            .and_then(|titles| titles.into_iter().next())
            .filter(|title| !title.is_empty())
    }

    /// 返回页面全文文本。
    ///
    /// 结果会合并所有文本节点，并把连续空白清理成单个空格。
    #[must_use]
    pub fn text(&self) -> String {
        clean_text(self.document.root_element().text())
    }

    /// 返回页面中的所有链接。
    ///
    /// 提取 `a[href]`，保留页面出现顺序，不去重、不过滤协议、不限制域名。有基础 URL
    /// 时会把相对链接补成绝对 URL。
    #[must_use]
    pub fn links(&self) -> Vec<String> {
        select_attr_from_document(&self.document, self.base_url.as_ref(), "a[href]", "href")
    }

    /// 返回页面中的所有图片地址。
    ///
    /// 提取 `img[src]`，保留页面出现顺序。有基础 URL 时会把相对地址补成绝对 URL。
    #[must_use]
    pub fn images(&self) -> Vec<String> {
        select_attr_from_document(&self.document, self.base_url.as_ref(), "img[src]", "src")
    }

    /// 使用 CSS selector 查询元素。
    ///
    /// selector 非法时返回 [`ErrorKind::Selector`]。返回的 [`Element`] 是拥有型数据，
    /// 不携带底层 DOM 生命周期。
    pub fn select(&self, selector: impl AsRef<str>) -> Result<Vec<Element>> {
        let selector = selector.as_ref();
        let parsed = parse_selector("select", selector)?;
        Ok(self
            .document
            .select(&parsed)
            .map(|element| Element::from_ref(element, self.base_url.as_ref()))
            .collect())
    }

    /// 使用 CSS selector 查询元素文本。
    ///
    /// 每个匹配元素返回一条清理后的文本。
    pub fn select_text(&self, selector: impl AsRef<str>) -> Result<Vec<String>> {
        Ok(self
            .select(selector)?
            .into_iter()
            .map(|element| element.text())
            .collect())
    }

    /// 使用 CSS selector 查询元素属性。
    ///
    /// 只返回存在该属性的元素值；缺少属性的元素会被跳过。
    pub fn select_attr(
        &self,
        selector: impl AsRef<str>,
        attr: impl AsRef<str>,
    ) -> Result<Vec<String>> {
        let attr = attr.as_ref().to_owned();
        Ok(self
            .select(selector)?
            .into_iter()
            .filter_map(|element| element.attr(&attr))
            .collect())
    }

    /// 使用 CSS selector 查询元素 HTML。
    ///
    /// 返回每个匹配元素自身的 HTML。
    pub fn select_html(&self, selector: impl AsRef<str>) -> Result<Vec<String>> {
        Ok(self
            .select(selector)?
            .into_iter()
            .map(|element| element.html())
            .collect())
    }

    /// 按 `id` 属性查询元素。
    ///
    /// 等价于常见的 `#id` 用法，但会安全处理属性值中的特殊字符。
    pub fn by_id(&self, id: impl AsRef<str>) -> Result<Vec<Element>> {
        self.select(attr_selector("id", id.as_ref()))
    }

    /// 按 class 名查询元素。
    ///
    /// 匹配 class 列表中的单个名字，适合常见的 `.item` 场景。
    pub fn by_class(&self, name: impl AsRef<str>) -> Result<Vec<Element>> {
        self.select(class_selector(name.as_ref()))
    }

    /// 按标签名查询元素。
    ///
    /// 标签名必须是简单的 HTML 标签名；复杂查询请使用 [`Page::select`]。
    pub fn by_tag(&self, name: impl AsRef<str>) -> Result<Vec<Element>> {
        let name = name.as_ref();
        simple_name_selector("by_tag", name)?;
        self.select(name)
    }

    /// 按 `name` 属性查询元素。
    ///
    /// 常用于表单字段，例如 `page.by_name("email")?`。
    pub fn by_name(&self, name: impl AsRef<str>) -> Result<Vec<Element>> {
        self.select(attr_selector("name", name.as_ref()))
    }
}

/// 高层 HTML 元素。
///
/// `Element` 保存匹配元素的 HTML、文本和属性。它可以继续在元素内部做 selector 查询，
/// 但不会暴露底层 DOM 节点。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Element {
    tag: String,
    html: String,
    inner_html: String,
    text: String,
    attrs: Vec<(String, String)>,
    base_url: Option<BaseUrl>,
}

impl Element {
    fn from_ref(element: ElementRef<'_>, base_url: Option<&BaseUrl>) -> Self {
        Self {
            tag: element.value().name().to_owned(),
            html: element.html(),
            inner_html: element.inner_html(),
            text: clean_text(element.text()),
            attrs: element
                .value()
                .attrs()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
            base_url: base_url.cloned(),
        }
    }

    /// 返回元素标签名。
    #[must_use]
    pub fn tag(&self) -> &str {
        &self.tag
    }

    /// 返回元素文本。
    ///
    /// 结果会清理连续空白。
    #[must_use]
    pub fn text(&self) -> String {
        self.text.clone()
    }

    /// 返回元素自身 HTML。
    #[must_use]
    pub fn html(&self) -> String {
        self.html.clone()
    }

    /// 返回元素内部 HTML。
    #[must_use]
    pub fn inner_html(&self) -> String {
        self.inner_html.clone()
    }

    /// 返回元素属性值。
    ///
    /// 属性不存在时返回 `None`。
    #[must_use]
    pub fn attr(&self, name: impl AsRef<str>) -> Option<String> {
        let name = name.as_ref();
        self.attrs
            .iter()
            .find(|(attr_name, _)| attr_name.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    /// 返回元素内部的所有链接。
    ///
    /// 如果当前元素本身是 `<a href="...">`，也会包含它自己的链接。
    #[must_use]
    pub fn links(&self) -> Vec<String> {
        let mut links = Vec::new();
        if self.tag.eq_ignore_ascii_case("a")
            && let Some(href) = self.attr("href")
        {
            links.push(resolve_url(&href, self.base_url.as_ref()));
        }
        links.extend(select_attr_from_html(
            &self.inner_html,
            self.base_url.as_ref(),
            "a[href]",
            "href",
        ));
        links
    }

    /// 返回元素内部的所有图片地址。
    ///
    /// 如果当前元素本身是 `<img src="...">`，也会包含它自己的图片地址。
    #[must_use]
    pub fn images(&self) -> Vec<String> {
        let mut images = Vec::new();
        if self.tag.eq_ignore_ascii_case("img")
            && let Some(src) = self.attr("src")
        {
            images.push(resolve_url(&src, self.base_url.as_ref()));
        }
        images.extend(select_attr_from_html(
            &self.inner_html,
            self.base_url.as_ref(),
            "img[src]",
            "src",
        ));
        images
    }

    /// 在当前元素内部使用 CSS selector 查询元素。
    ///
    /// selector 非法时返回 [`ErrorKind::Selector`]。
    pub fn select(&self, selector: impl AsRef<str>) -> Result<Vec<Element>> {
        let selector = selector.as_ref();
        let parsed = parse_selector("select", selector)?;
        let fragment = Html::parse_fragment(&self.inner_html);
        Ok(fragment
            .select(&parsed)
            .map(|element| Element::from_ref(element, self.base_url.as_ref()))
            .collect())
    }

    /// 在当前元素内部使用 CSS selector 查询文本。
    pub fn select_text(&self, selector: impl AsRef<str>) -> Result<Vec<String>> {
        Ok(self
            .select(selector)?
            .into_iter()
            .map(|element| element.text())
            .collect())
    }

    /// 在当前元素内部使用 CSS selector 查询属性。
    ///
    /// 只返回存在该属性的元素值。
    pub fn select_attr(
        &self,
        selector: impl AsRef<str>,
        attr: impl AsRef<str>,
    ) -> Result<Vec<String>> {
        let attr = attr.as_ref().to_owned();
        Ok(self
            .select(selector)?
            .into_iter()
            .filter_map(|element| element.attr(&attr))
            .collect())
    }

    /// 在当前元素内部使用 CSS selector 查询 HTML。
    pub fn select_html(&self, selector: impl AsRef<str>) -> Result<Vec<String>> {
        Ok(self
            .select(selector)?
            .into_iter()
            .map(|element| element.html())
            .collect())
    }
}

/// 下载并解析 HTML 页面。
///
/// 请求使用 easy-rust 的 [`request`] 模块默认配置。HTTP 404/500 等状态会通过
/// `raise_for_status()` 转成错误；成功时返回 [`Page`]。
pub async fn get(url: impl AsRef<str>) -> Result<Page> {
    let url = url.as_ref().to_owned();
    let response = request::get(&url).await.map_err(|source| {
        Error::with_source(
            ErrorKind::Fetch {
                operation: "get",
                url: url.clone(),
            },
            source,
        )
    })?;
    response.raise_for_status().map_err(|source| {
        Error::with_source(
            ErrorKind::Fetch {
                operation: "get",
                url: url.clone(),
            },
            source,
        )
    })?;
    from_html_with_url(response.url(), response.text())
}

/// 解析 HTML 字符串。
///
/// 这个入口不绑定基础 URL，因此 [`Page::links`] 和 [`Page::images`] 会返回页面里的原始地址。
#[must_use]
pub fn from_html(html: impl Into<String>) -> Page {
    let html = html.into();
    let document = Html::parse_document(&html);
    Page {
        html,
        document,
        url: None,
        base_url: None,
    }
}

/// 解析 HTML 字符串并设置基础 URL。
///
/// 基础 URL 非法时返回 [`ErrorKind::Url`]。设置基础 URL 后，链接和图片地址会尽量补成绝对
/// URL。
pub fn from_html_with_url(url: impl AsRef<str>, html: impl Into<String>) -> Result<Page> {
    let url = url.as_ref();
    let base_url = BaseUrl::parse(url).map_err(|source| {
        Error::with_source(
            ErrorKind::Url {
                operation: "from_html_with_url",
                url: url.to_owned(),
            },
            source,
        )
    })?;
    let html = html.into();
    let document = Html::parse_document(&html);
    Ok(Page {
        html,
        document,
        url: Some(base_url.as_str().to_owned()),
        base_url: Some(base_url),
    })
}

fn parse_selector(operation: &'static str, selector: &str) -> Result<Selector> {
    Selector::parse(selector).map_err(|_| {
        Error::new(ErrorKind::Selector {
            operation,
            selector: selector.to_owned(),
        })
    })
}

fn select_attr_from_document(
    document: &Html,
    base_url: Option<&BaseUrl>,
    selector: &str,
    attr: &str,
) -> Vec<String> {
    let Ok(selector) = Selector::parse(selector) else {
        return Vec::new();
    };
    document
        .select(&selector)
        .filter_map(|element| element.value().attr(attr))
        .map(|value| resolve_url(value, base_url))
        .collect()
}

fn select_attr_from_html(
    html: &str,
    base_url: Option<&BaseUrl>,
    selector: &str,
    attr: &str,
) -> Vec<String> {
    let Ok(selector) = Selector::parse(selector) else {
        return Vec::new();
    };
    let fragment = Html::parse_fragment(html);
    fragment
        .select(&selector)
        .filter_map(|element| element.value().attr(attr))
        .map(|value| resolve_url(value, base_url))
        .collect()
}

fn resolve_url(value: &str, base_url: Option<&BaseUrl>) -> String {
    if let Some(base_url) = base_url
        && let Ok(url) = base_url.join(value)
    {
        return url.to_string();
    }
    value.to_owned()
}

fn attr_selector(name: &str, value: &str) -> String {
    format!(r#"[{name}="{}"]"#, css_string(value))
}

fn class_selector(value: &str) -> String {
    format!(r#"[class~="{}"]"#, css_string(value))
}

fn css_string(value: &str) -> String {
    let mut escaped = String::new();
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str(r"\\"),
            '"' => escaped.push_str(r#"\""#),
            '\n' => escaped.push_str(r"\a "),
            '\r' => escaped.push_str(r"\d "),
            '\t' => escaped.push_str(r"\9 "),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn simple_name_selector(operation: &'static str, name: &str) -> Result<()> {
    let mut chars = name.chars();
    let valid = chars
        .next()
        .is_some_and(|first| first.is_ascii_alphabetic())
        && chars.all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | ':'));

    if valid {
        Ok(())
    } else {
        Err(Error::new(ErrorKind::Selector {
            operation,
            selector: name.to_owned(),
        }))
    }
}

fn clean_text<'a>(texts: impl IntoIterator<Item = &'a str>) -> String {
    let mut cleaned = String::new();
    for text in texts {
        for word in text.split_whitespace() {
            if !cleaned.is_empty() {
                cleaned.push(' ');
            }
            cleaned.push_str(word);
        }
    }
    cleaned
}

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{method, path},
    };

    use super::*;

    const HTML: &str = r#"
        <!doctype html>
        <html>
            <head><title> Demo  Page </title></head>
            <body>
                <main id="app" class="page main" name="root">
                    <h1 class="title">Hello   Rust</h1>
                    <a class="nav" href="/next"> Next </a>
                    <img src="img/logo.png" alt="Logo">
                    <article class="item" data-id="1"><span>Ada</span></article>
                    <article class="item" data-id="2"><span>Grace</span></article>
                    <input name="email" value="ada@example.com">
                </main>
            </body>
        </html>
    "#;

    #[test]
    fn from_html_reads_title_text_links_and_images() {
        let page = from_html(HTML);

        assert_eq!(page.url(), None);
        assert_eq!(page.title(), Some("Demo Page".to_owned()));
        assert!(page.text().contains("Hello Rust"));
        assert_eq!(page.links(), vec!["/next".to_owned()]);
        assert_eq!(page.images(), vec!["img/logo.png".to_owned()]);
    }

    #[test]
    fn from_html_with_url_resolves_relative_assets() -> std::result::Result<(), Box<dyn StdError>> {
        let page = from_html_with_url("https://example.com/docs/page.html", HTML)?;

        assert_eq!(page.links(), vec!["https://example.com/next".to_owned()]);
        assert_eq!(
            page.images(),
            vec!["https://example.com/docs/img/logo.png".to_owned()]
        );
        Ok(())
    }

    #[test]
    fn selectors_return_text_attr_and_html() -> std::result::Result<(), Box<dyn StdError>> {
        let page = from_html(HTML);

        assert_eq!(
            page.select_text(".item span")?,
            vec!["Ada".to_owned(), "Grace".to_owned()]
        );
        assert_eq!(
            page.select_attr(".item", "data-id")?,
            vec!["1".to_owned(), "2".to_owned()]
        );
        let html = page.select_html("h1.title")?;
        assert_eq!(html.len(), 1);
        assert!(html[0].contains("Hello"));
        Ok(())
    }

    #[test]
    fn convenience_selectors_work() -> std::result::Result<(), Box<dyn StdError>> {
        let page = from_html(HTML);

        assert_eq!(page.by_id("app")?.len(), 1);
        assert_eq!(page.by_class("item")?.len(), 2);
        assert_eq!(page.by_tag("article")?.len(), 2);
        assert_eq!(
            page.by_name("email")?
                .into_iter()
                .next()
                .and_then(|element| element.attr("value")),
            Some("ada@example.com".to_owned())
        );
        Ok(())
    }

    #[test]
    fn element_methods_work() -> std::result::Result<(), Box<dyn StdError>> {
        let page = from_html_with_url("https://example.com/base/", HTML)?;
        let card = page
            .by_id("app")?
            .into_iter()
            .next()
            .ok_or("missing app element")?;

        assert_eq!(card.tag(), "main");
        assert_eq!(card.attr("name"), Some("root".to_owned()));
        assert!(card.inner_html().contains("article"));
        assert!(card.html().contains("id=\"app\""));
        assert!(card.text().contains("Ada"));
        assert_eq!(card.links(), vec!["https://example.com/next".to_owned()]);
        assert_eq!(
            card.images(),
            vec!["https://example.com/base/img/logo.png".to_owned()]
        );
        assert_eq!(
            card.select_text("article span")?,
            vec!["Ada".to_owned(), "Grace".to_owned()]
        );
        assert_eq!(
            card.select_attr("input", "value")?,
            vec!["ada@example.com".to_owned()]
        );
        assert_eq!(card.select_html("article")?.len(), 2);
        Ok(())
    }

    #[test]
    fn invalid_selector_returns_context_error() -> std::result::Result<(), Box<dyn StdError>> {
        let page = from_html(HTML);
        let error = match page.select("[") {
            Ok(value) => return Err(format!("expected selector error, got {}", value.len()).into()),
            Err(error) => error,
        };

        assert!(error.to_string().contains("select"));
        assert!(error.to_string().contains("["));
        Ok(())
    }

    #[tokio::test]
    async fn get_fetches_and_parses_page() -> std::result::Result<(), Box<dyn StdError>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/page"))
            .respond_with(ResponseTemplate::new(200).set_body_string(HTML))
            .mount(&server)
            .await;

        let page = get(format!("{}/page", server.uri())).await?;

        assert_eq!(page.title(), Some("Demo Page".to_owned()));
        assert_eq!(page.links(), vec![format!("{}/next", server.uri())]);
        assert_eq!(page.select_text(".title")?, vec!["Hello Rust".to_owned()]);
        Ok(())
    }

    #[tokio::test]
    async fn get_returns_status_error_with_context() -> std::result::Result<(), Box<dyn StdError>> {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/missing"))
            .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
            .mount(&server)
            .await;

        let url = format!("{}/missing", server.uri());
        let error = match get(&url).await {
            Ok(page) => return Err(format!("expected fetch error, got {:?}", page.url()).into()),
            Err(error) => error,
        };

        let message = error.to_string();
        assert!(message.contains("get"));
        assert!(message.contains(&url));
        assert!(message.contains("404"));
        Ok(())
    }
}
