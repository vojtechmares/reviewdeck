//! A thin HTTP client with the error handling every provider adapter wants: a port
//! of src/main/http.ts.
//!
//! The real transport is reqwest (Zed's fork, which gpui already compiles) on a
//! private single-threaded tokio runtime that runs on its own thread. Every request,
//! body included, runs there as a task and the caller awaits its join handle, so the
//! futures this module returns can be awaited from any executor - gpui's included -
//! without that executor knowing anything about tokio. Dropping one of those futures
//! cancels the request, the way aborting a `fetch` does.
//!
//! [`Http::mock`] swaps the transport for a function, so everything above it can be
//! tested without a network.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, LazyLock, OnceLock};
use std::task::{Context, Poll};
use std::time::Duration;

use parking_lot::Mutex;
use regex::Regex;
use serde::de::{DeserializeOwned, IgnoredAny};
use serde_json::Value;
use url::Url;

use crate::error::{Error, Result, msg};

/// How long a request may take before it is abandoned.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// How many redirects a request follows unless told otherwise; `fetch` stops at 20.
const DEFAULT_MAX_REDIRECTS: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum Method {
    #[default]
    Get,
    Post,
    Put,
    Patch,
    Delete,
}

impl Method {
    pub fn as_str(self) -> &'static str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
        }
    }

    fn to_reqwest(self) -> reqwest::Method {
        match self {
            Method::Get => reqwest::Method::GET,
            Method::Post => reqwest::Method::POST,
            Method::Put => reqwest::Method::PUT,
            Method::Patch => reqwest::Method::PATCH,
            Method::Delete => reqwest::Method::DELETE,
        }
    }
}

/// A request body, and how it is encoded on the wire.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// Sent as `application/json`.
    Json(Value),
    /// Sent as `application/x-www-form-urlencoded` through [`encode_form`]
    /// (GitLab discussions want this).
    Form(Value),
}

#[derive(Debug, Clone, Default)]
pub struct RequestOptions {
    pub method: Method,
    pub headers: Vec<(String, String)>,
    pub body: Option<Body>,
    /// `None` is [`DEFAULT_TIMEOUT`]. Covers the whole exchange, body included.
    pub timeout: Option<Duration>,
    /// `None` follows up to 20 redirects, as `fetch` does. `Some(0)` follows none and
    /// hands the 3xx back - for [`Http::send`] callers that follow redirects by hand,
    /// as the image proxy does, so a credential stops at the hosts it belongs to.
    pub max_redirects: Option<usize>,
}

impl RequestOptions {
    pub fn new(method: Method) -> Self {
        RequestOptions {
            method,
            ..RequestOptions::default()
        }
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn headers<K: Into<String>, V: Into<String>>(
        mut self,
        headers: impl IntoIterator<Item = (K, V)>,
    ) -> Self {
        self.headers
            .extend(headers.into_iter().map(|(k, v)| (k.into(), v.into())));
        self
    }

    pub fn json(mut self, body: Value) -> Self {
        self.body = Some(Body::Json(body));
        self
    }

    pub fn form(mut self, body: Value) -> Self {
        self.body = Some(Body::Form(body));
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    pub fn max_redirects(mut self, max_redirects: usize) -> Self {
        self.max_redirects = Some(max_redirects);
        self
    }
}

/// A response with its body read in full.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Response {
    pub status: u16,
    /// Header names in lower case, in the order the host sent them.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Response {
    /// A 2xx status: `Response#ok`.
    pub fn is_ok(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// A header by name, any case, as `Headers#get` reads it: every value under that
    /// name joined with `", "`, or `None` when there is none.
    pub fn header(&self, name: &str) -> Option<String> {
        let values: Vec<&str> = self
            .headers
            .iter()
            .filter(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
            .collect();
        (!values.is_empty()).then(|| values.join(", "))
    }

    /// The body as text, the way `Response#text` decodes it: UTF-8 whatever the
    /// headers say, a leading byte order mark dropped, bad sequences replaced.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(strip_bom(&self.body)).into_owned()
    }
}

/// What a mock transport is asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockRequest {
    pub method: Method,
    pub url: String,
    /// Exactly the headers that would go on the wire, `Content-Type` included.
    pub headers: Vec<(String, String)>,
    /// The encoded body.
    pub body: Option<String>,
}

impl MockRequest {
    /// A header by name, any case.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    /// The body parsed as JSON, when it is JSON.
    pub fn json_body(&self) -> Option<Value> {
        serde_json::from_str(self.body.as_deref()?).ok()
    }
}

/// What a mock transport answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Fail the way a transport does instead of answering.
    pub failure: Option<MockFailure>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockFailure {
    /// The host could not be reached; the reason is what the transport would say.
    Unreachable(String),
    /// The request outlived its timeout.
    TimedOut,
}

impl MockResponse {
    pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
        MockResponse {
            status,
            headers: Vec::new(),
            body: body.into(),
            failure: None,
        }
    }

    /// A JSON body, with its `Content-Type`.
    pub fn json(status: u16, body: &Value) -> Self {
        MockResponse::new(status, body.to_string()).header("content-type", "application/json")
    }

    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push((name.into(), value.into()));
        self
    }

    pub fn unreachable(reason: impl Into<String>) -> Self {
        MockResponse {
            failure: Some(MockFailure::Unreachable(reason.into())),
            ..MockResponse::new(0, Vec::new())
        }
    }

    pub fn timed_out() -> Self {
        MockResponse {
            failure: Some(MockFailure::TimedOut),
            ..MockResponse::new(0, Vec::new())
        }
    }
}

type MockHandler = dyn Fn(&MockRequest) -> MockResponse + Send + Sync;

/// The HTTP client. Cheap to clone: everything lives behind an `Arc`.
#[derive(Clone)]
pub struct Http {
    transport: Transport,
}

#[derive(Clone)]
enum Transport {
    Real(Arc<Real>),
    Mock(Arc<MockHandler>),
}

impl Default for Http {
    fn default() -> Self {
        Http::new()
    }
}

impl std::fmt::Debug for Http {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self.transport {
            Transport::Real(_) => "Http(real)",
            Transport::Mock(_) => "Http(mock)",
        })
    }
}

impl Http {
    /// The real client. The runtime thread behind it is shared by every `Http`, and
    /// started the first time one is made.
    pub fn new() -> Http {
        Http {
            transport: Transport::Real(Arc::new(Real {
                runtime: runtime().clone(),
                clients: Mutex::new(HashMap::new()),
            })),
        }
    }

    /// A client whose every request is answered by `handler`. For tests.
    pub fn mock(handler: impl Fn(&MockRequest) -> MockResponse + Send + Sync + 'static) -> Http {
        Http {
            transport: Transport::Mock(Arc::new(handler)),
        }
    }

    /// Sends a request and returns whatever came back, whatever its status. Only a
    /// transport failure is an error: the host unreachable, or the timeout passed.
    pub async fn send(&self, url: &str, options: RequestOptions) -> Result<Response> {
        let outgoing = Outgoing::new(url, options);
        match &self.transport {
            Transport::Real(real) => Arc::clone(real).send(outgoing).await,
            Transport::Mock(handler) => mock_send(handler.as_ref(), outgoing),
        }
    }

    /// Sends a request and insists on a 2xx: anything else becomes an
    /// [`Error::Api`] carrying the sentence [`describe`] makes of it.
    pub async fn request(&self, url: &str, options: RequestOptions) -> Result<Response> {
        let response = self.send(url, options).await?;
        if !response.is_ok() {
            return Err(api_error(url, &response));
        }
        Ok(response)
    }

    /// A 2xx response read as JSON.
    ///
    /// As in the TypeScript, a 204 or an empty body reads as JSON `null` (so ask for
    /// an `Option`, `()` or a `Value` where the host may send nothing), and a body
    /// that is not JSON at all reads as the JSON string of its text.
    pub async fn json<T: DeserializeOwned>(&self, url: &str, options: RequestOptions) -> Result<T> {
        let response = self.request(url, options).await?;
        decode_json(url, &response)
    }

    /// A 2xx response's body as text, whatever it is.
    pub async fn text(&self, url: &str, options: RequestOptions) -> Result<String> {
        Ok(self.request(url, options).await?.text())
    }

    /// Follows RFC 5988 `Link: <...>; rel="next"` pagination, capped at `max_pages`
    /// so a huge account cannot hang a sync. Each page is a GET with the options'
    /// headers; a page that is not a JSON array adds nothing.
    pub async fn paginate<T: DeserializeOwned>(
        &self,
        url: &str,
        options: RequestOptions,
        max_pages: usize,
    ) -> Result<Vec<T>> {
        let mut results = Vec::new();
        let mut next = Some(url.to_string());
        let mut page = 0;
        while page < max_pages
            && let Some(current) = next.take()
        {
            let page_options = RequestOptions {
                method: Method::Get,
                headers: options.headers.clone(),
                body: None,
                timeout: options.timeout,
                max_redirects: options.max_redirects,
            };
            let response = self.send(&current, page_options).await?;
            if !response.is_ok() {
                return Err(api_error(&current, &response));
            }
            let body = strip_bom(&response.body);
            match serde_json::from_slice::<Vec<T>>(body) {
                Ok(batch) => results.extend(batch),
                Err(error) => match serde_json::from_slice::<Value>(body) {
                    // An array, but one whose entries are not what was asked for.
                    Ok(Value::Array(_)) => return Err(unexpected(&current, &error)),
                    // Valid JSON that is not an array: nothing to add.
                    Ok(_) => {}
                    Err(error) => return Err(unexpected(&current, &error)),
                },
            }
            next = parse_next_link(response.header("link").as_deref());
            page += 1;
        }
        Ok(results)
    }
}

/// The sentence the user reads for a non-2xx response.
///
/// Providers put the useful part of an error in different fields, so after the
/// statuses that say enough on their own, the common ones are tried in turn.
pub fn describe(status: u16, url: &str, body: &str) -> String {
    static RATE_LIMIT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new("(?i)rate limit").expect("valid pattern"));

    let host = safe_host(url);
    if status == 401 {
        return format!("Not authorised on {host} - the token is invalid or expired.");
    }
    if status == 403 {
        if RATE_LIMIT.is_match(body) {
            return format!("Rate limited by {host}. Try again shortly.");
        }
        return format!("Forbidden on {host} - the token is missing a required scope.");
    }
    if status == 404 {
        let path = Url::parse(url).map_or_else(|_| url.to_string(), |url| url.path().to_string());
        return format!("Not found on {host} ({path}).");
    }
    if status >= 500 {
        return format!("{host} returned a server error ({status}).");
    }

    // `message ?? error ?? error_description ?? errors`: the first one present and
    // not null wins, whatever its type, and only a string or a non-empty array of
    // something says anything.
    if let Ok(Value::Object(parsed)) = serde_json::from_str::<Value>(body) {
        let detail = ["message", "error", "error_description", "errors"]
            .iter()
            .find_map(|key| parsed.get(*key).filter(|value| !value.is_null()));
        match detail {
            Some(Value::String(detail)) => return detail.clone(),
            Some(Value::Array(detail)) if !detail.is_empty() => return js_string(&detail[0]),
            _ => {}
        }
    }
    format!("{host} returned {status}.")
}

/// The host (and port, when it is not the scheme's default) of a URL, or the input
/// itself when it is not a URL: `new URL(url).host`.
pub fn safe_host(url: &str) -> String {
    match Url::parse(url) {
        Ok(url) => host_of(&url),
        Err(_) => url.to_string(),
    }
}

fn host_of(url: &Url) -> String {
    let host = url.host_str().unwrap_or("");
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_string(),
    }
}

/// The URL in a `Link` header's `rel="next"` entry, if there is one.
pub fn parse_next_link(header: Option<&str>) -> Option<String> {
    static NEXT: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"<([^>]+)>\s*;\s*rel="?next"?"#).expect("valid pattern"));
    let header = header?;
    if header.is_empty() {
        return None;
    }
    header.split(',').find_map(|part| {
        NEXT.captures(js_trim(part))
            .and_then(|captures| captures.get(1))
            .map(|url| url.as_str().to_string())
    })
}

/// Normalises whatever the user typed into an origin: "gitlab.com" becomes
/// "https://gitlab.com". A path is kept, for instances hosted under one.
pub fn to_origin(input: &str) -> Result<String> {
    let trimmed = js_trim(input).trim_end_matches('/');
    if trimmed.is_empty() {
        return Err(msg("A host is required."));
    }
    let lower = trimmed.get(..8).unwrap_or(trimmed).to_ascii_lowercase();
    let with_scheme = if lower.starts_with("http://") || lower.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("https://{trimmed}")
    };
    // The message `new URL` throws with.
    let url = Url::parse(&with_scheme).map_err(|_| msg("Invalid URL"))?;
    let path = match url.path() {
        "/" => "",
        path => path.trim_end_matches('/'),
    };
    Ok(format!("{}://{}{}", url.scheme(), host_of(&url), path))
}

/// Flattens `{ position: { new_line: 3 } }` into `position%5Bnew_line%5D=3`, the
/// bracket notation Rails-based APIs (GitLab) expect from form posts. Arrays become
/// repeated `name[]` pairs and `null` entries are left out.
pub fn encode_form(source: &Value) -> String {
    encode_form_under(source, "")
}

fn encode_form_under(source: &Value, prefix: &str) -> String {
    let Value::Object(entries) = source else {
        return String::new();
    };
    let mut parts = Vec::new();
    for (key, value) in entries {
        let name = if prefix.is_empty() {
            key.clone()
        } else {
            format!("{prefix}[{key}]")
        };
        match value {
            Value::Null => {}
            Value::Object(_) => {
                let nested = encode_form_under(value, &name);
                if !nested.is_empty() {
                    parts.push(nested);
                }
            }
            Value::Array(list) => {
                let name = encode_uri_component(&format!("{name}[]"));
                for entry in list {
                    parts.push(format!(
                        "{name}={}",
                        encode_uri_component(&js_string(entry))
                    ));
                }
            }
            _ => parts.push(format!(
                "{}={}",
                encode_uri_component(&name),
                encode_uri_component(&js_string(value))
            )),
        }
    }
    parts.join("&")
}

/// `encodeURIComponent`: everything but `A-Z a-z 0-9 - _ . ! ~ * ' ( )` is
/// percent-encoded as UTF-8.
fn encode_uri_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for byte in input.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(byte as char),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// `String(value)` for a JSON value.
fn js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(value) => value.to_string(),
        Value::Number(number) => js_number(number),
        Value::String(text) => text.clone(),
        Value::Array(list) => list
            .iter()
            .map(|entry| match entry {
                Value::Null => String::new(),
                entry => js_string(entry),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".into(),
    }
}

/// `Number#toString()`.
fn js_number(number: &serde_json::Number) -> String {
    if let Some(integer) = number.as_i64() {
        return integer.to_string();
    }
    if let Some(integer) = number.as_u64() {
        return integer.to_string();
    }
    let Some(value) = number.as_f64() else {
        return number.to_string();
    };
    if value == 0.0 {
        return "0".into();
    }
    // The shortest digits that round-trip, and the decimal exponent, then laid out
    // by the rules of ECMA-262 Number::toString.
    let scientific = format!("{:e}", value.abs());
    let Some((mantissa, exponent)) = scientific.split_once('e') else {
        return scientific;
    };
    let Ok(exponent) = exponent.parse::<i32>() else {
        return scientific;
    };
    let digits: String = mantissa.chars().filter(|c| *c != '.').collect();
    let k = digits.len() as i32;
    let n = exponent + 1;
    let body = if k <= n && n <= 21 {
        format!("{digits}{}", "0".repeat((n - k) as usize))
    } else if 0 < n && n <= 21 {
        format!("{}.{}", &digits[..n as usize], &digits[n as usize..])
    } else if -6 < n && n <= 0 {
        format!("0.{}{digits}", "0".repeat((-n) as usize))
    } else {
        let sign = if n > 0 { '+' } else { '-' };
        let magnitude = (n - 1).abs();
        if k == 1 {
            format!("{digits}e{sign}{magnitude}")
        } else {
            format!("{}.{}e{sign}{magnitude}", &digits[..1], &digits[1..])
        }
    };
    if value < 0.0 {
        format!("-{body}")
    } else {
        body
    }
}

/// `String#trim`: JavaScript's white space and line terminators, which are not
/// quite Unicode's (U+FEFF is in, U+0085 is out).
fn js_trim(input: &str) -> &str {
    input.trim_matches(|c: char| {
        matches!(
            c,
            '\t' | '\n' | '\u{B}' | '\u{C}' | '\r' | ' ' | '\u{A0}' | '\u{1680}' | '\u{2000}'
                ..='\u{200A}'
                    | '\u{2028}'
                    | '\u{2029}'
                    | '\u{202F}'
                    | '\u{205F}'
                    | '\u{3000}'
                    | '\u{FEFF}'
        )
    })
}

fn strip_bom(body: &[u8]) -> &[u8] {
    body.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(body)
}

fn decode_json<T: DeserializeOwned>(url: &str, response: &Response) -> Result<T> {
    let body = strip_bom(&response.body);
    if response.status == 204 || body.is_empty() {
        return T::deserialize(&Value::Null).map_err(|error| unexpected(url, &error));
    }
    match serde_json::from_slice::<T>(body) {
        Ok(value) => Ok(value),
        Err(error) => {
            let is_json = !error.is_syntax()
                && !error.is_eof()
                && serde_json::from_slice::<IgnoredAny>(body).is_ok();
            if is_json {
                return Err(unexpected(url, &error));
            }
            // Not JSON at all: the TypeScript hands back the text itself.
            let text = String::from_utf8_lossy(body).into_owned();
            T::deserialize(Value::String(text)).map_err(|_| unexpected(url, &error))
        }
    }
}

fn api_error(url: &str, response: &Response) -> Error {
    let text = response.text();
    Error::Api {
        status: response.status,
        url: url.to_string(),
        message: describe(response.status, url, &text),
        body: Some(text),
    }
}

fn unexpected(url: &str, error: &serde_json::Error) -> Error {
    msg(format!(
        "Unexpected response from {}: {error}",
        safe_host(url)
    ))
}

fn timed_out(url: &str) -> Error {
    Error::Api {
        status: 0,
        url: url.to_string(),
        message: format!("Request to {} timed out.", safe_host(url)),
        body: None,
    }
}

fn unreachable(url: &str, reason: &str) -> Error {
    Error::Api {
        status: 0,
        url: url.to_string(),
        message: format!("Could not reach {}: {reason}", safe_host(url)),
        body: None,
    }
}

/// A request as it goes on the wire.
struct Outgoing {
    method: Method,
    url: String,
    headers: Vec<(String, String)>,
    body: Option<String>,
    timeout: Duration,
    max_redirects: Option<usize>,
}

impl Outgoing {
    fn new(url: &str, options: RequestOptions) -> Outgoing {
        let mut headers = options.headers;
        let body = options.body.map(|body| {
            let (content_type, encoded) = match body {
                Body::Json(value) => ("application/json", value.to_string()),
                Body::Form(value) => ("application/x-www-form-urlencoded", encode_form(&value)),
            };
            headers.retain(|(name, _)| !name.eq_ignore_ascii_case("content-type"));
            headers.push(("Content-Type".into(), content_type.into()));
            encoded
        });
        Outgoing {
            method: options.method,
            url: url.to_string(),
            headers,
            body,
            timeout: options.timeout.unwrap_or(DEFAULT_TIMEOUT),
            max_redirects: options.max_redirects,
        }
    }
}

fn mock_send(handler: &MockHandler, outgoing: Outgoing) -> Result<Response> {
    let request = MockRequest {
        method: outgoing.method,
        url: outgoing.url,
        headers: outgoing.headers,
        body: outgoing.body,
    };
    let response = handler(&request);
    match response.failure {
        Some(MockFailure::Unreachable(reason)) => Err(unreachable(&request.url, &reason)),
        Some(MockFailure::TimedOut) => Err(timed_out(&request.url)),
        None => Ok(Response {
            status: response.status,
            headers: response
                .headers
                .into_iter()
                .map(|(name, value)| (name.to_ascii_lowercase(), value))
                .collect(),
            body: response.body,
        }),
    }
}

/// The real transport: a reqwest client per redirect policy (reqwest fixes the
/// policy per client), built on first use and reused so connections pool.
struct Real {
    runtime: std::result::Result<tokio::runtime::Handle, String>,
    clients: Mutex<HashMap<Option<usize>, reqwest::Client>>,
}

impl Real {
    async fn send(self: Arc<Self>, outgoing: Outgoing) -> Result<Response> {
        let url = outgoing.url.clone();
        let handle = match &self.runtime {
            Ok(handle) => handle.clone(),
            Err(reason) => return Err(unreachable(&url, reason)),
        };
        let task = AbortOnDrop(handle.spawn(async move { self.perform(outgoing).await }));
        match task.await {
            Ok(result) => result,
            Err(error) => Err(unreachable(&url, &error.to_string())),
        }
    }

    /// Runs on the private runtime.
    async fn perform(&self, outgoing: Outgoing) -> Result<Response> {
        let url = outgoing.url;
        let client = self
            .client(outgoing.max_redirects)
            .map_err(|reason| unreachable(&url, &reason))?;
        let mut builder = client.request(outgoing.method.to_reqwest(), url.as_str());
        for (name, value) in &outgoing.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if let Some(body) = outgoing.body {
            builder = builder.body(body);
        }

        let exchange = async {
            let response = builder.send().await?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .map(|(name, value)| {
                    let value = String::from_utf8_lossy(value.as_bytes()).into_owned();
                    (name.as_str().to_string(), value)
                })
                .collect();
            let body = match response.bytes().await {
                Ok(bytes) => bytes.to_vec(),
                // An error status still says what went wrong without its body, as
                // `response.text().catch(() => '')` has it.
                Err(_) if !(200..300).contains(&status) => Vec::new(),
                Err(error) => return Err(error),
            };
            Ok(Response {
                status,
                headers,
                body,
            })
        };

        match tokio::time::timeout(outgoing.timeout, exchange).await {
            Err(_) => Err(timed_out(&url)),
            Ok(Err(error)) if error.is_timeout() => Err(timed_out(&url)),
            Ok(Err(error)) => Err(unreachable(&url, &root_cause(&error))),
            Ok(Ok(response)) => Ok(response),
        }
    }

    fn client(&self, max_redirects: Option<usize>) -> std::result::Result<reqwest::Client, String> {
        let mut clients = self.clients.lock();
        if let Some(client) = clients.get(&max_redirects) {
            return Ok(client.clone());
        }
        let policy = match max_redirects {
            Some(0) => reqwest::redirect::Policy::none(),
            Some(max) => reqwest::redirect::Policy::limited(max),
            None => reqwest::redirect::Policy::limited(DEFAULT_MAX_REDIRECTS),
        };
        let client = reqwest::Client::builder()
            .redirect_policy(policy)
            .build()
            .map_err(|error| root_cause(&error))?;
        clients.insert(max_redirects, client.clone());
        Ok(client)
    }
}

/// The innermost error in a chain, which is the one that says what happened
/// ("Connection refused (os error 61)") rather than where.
fn root_cause(error: &(dyn std::error::Error + 'static)) -> String {
    let mut current = error;
    while let Some(source) = current.source() {
        current = source;
    }
    current.to_string()
}

/// The private runtime, started once on its own thread.
fn runtime() -> &'static std::result::Result<tokio::runtime::Handle, String> {
    static RUNTIME: OnceLock<std::result::Result<tokio::runtime::Handle, String>> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .thread_name("reviewdeck-http")
            .build()
            .map_err(|error| error.to_string())?;
        let handle = runtime.handle().clone();
        std::thread::Builder::new()
            .name("reviewdeck-http".into())
            .spawn(move || runtime.block_on(std::future::pending::<()>()))
            .map_err(|error| error.to_string())?;
        Ok(handle)
    })
}

/// A join handle that cancels its task when dropped, so abandoning a request
/// abandons it on the runtime too.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Future for AbortOnDrop<T> {
    type Output = std::result::Result<T, tokio::task::JoinError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.get_mut().0).poll(cx)
    }
}

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::executor::block_on;
    use serde_json::json;

    // --- test/http.test.ts ---

    #[test]
    fn parse_next_link_picks_the_next_relation_out_of_a_link_header() {
        let header = r#"<https://api.github.com/x?page=2>; rel="next", <https://api.github.com/x?page=9>; rel="last""#;
        assert_eq!(
            parse_next_link(Some(header)).as_deref(),
            Some("https://api.github.com/x?page=2")
        );
    }

    #[test]
    fn parse_next_link_returns_none_when_there_is_no_next_page() {
        assert_eq!(
            parse_next_link(Some(r#"<https://api.github.com/x?page=1>; rel="prev""#)),
            None
        );
        assert_eq!(parse_next_link(None), None);
        assert_eq!(parse_next_link(Some("")), None);
        // Unquoted, as some hosts send it.
        assert_eq!(
            parse_next_link(Some("<https://h.test/p2>; rel=next")).as_deref(),
            Some("https://h.test/p2")
        );
    }

    #[test]
    fn to_origin_normalises_whatever_the_user_typed() {
        assert_eq!(to_origin("gitlab.com").unwrap(), "https://gitlab.com");
        assert_eq!(
            to_origin("https://gitlab.com/").unwrap(),
            "https://gitlab.com"
        );
        assert_eq!(
            to_origin("  git.acme.dev  ").unwrap(),
            "https://git.acme.dev"
        );
        // A path prefix matters for instances hosted under a sub-path.
        assert_eq!(
            to_origin("https://acme.dev/git/").unwrap(),
            "https://acme.dev/git"
        );
        assert_eq!(
            to_origin("http://localhost:3000").unwrap(),
            "http://localhost:3000"
        );
        assert_eq!(
            to_origin("HTTPS://GitLab.com").unwrap(),
            "https://gitlab.com"
        );
        assert_eq!(
            to_origin("https://gitlab.com:443").unwrap(),
            "https://gitlab.com"
        );
    }

    #[test]
    fn to_origin_rejects_an_empty_host() {
        let error = to_origin("   ").unwrap_err();
        assert!(error.to_string().contains("host is required"), "{error}");
        assert!(to_origin("///").is_err());
    }

    // --- describe ---

    #[test]
    fn describe_speaks_for_the_statuses_that_say_enough_on_their_own() {
        let url = "https://gitlab.example.com/api/v4/projects/7";
        assert_eq!(
            describe(401, url, ""),
            "Not authorised on gitlab.example.com - the token is invalid or expired."
        );
        assert_eq!(
            describe(403, url, r#"{"message":"API rate limit exceeded"}"#),
            "Rate limited by gitlab.example.com. Try again shortly."
        );
        assert_eq!(
            describe(403, url, "Rate Limit hit"),
            "Rate limited by gitlab.example.com. Try again shortly."
        );
        assert_eq!(
            describe(403, url, r#"{"message":"insufficient_scope"}"#),
            "Forbidden on gitlab.example.com - the token is missing a required scope."
        );
        assert_eq!(
            describe(404, url, ""),
            "Not found on gitlab.example.com (/api/v4/projects/7)."
        );
        assert_eq!(
            describe(500, url, r#"{"message":"boom"}"#),
            "gitlab.example.com returned a server error (500)."
        );
        assert_eq!(
            describe(503, "http://localhost:8080/x", ""),
            "localhost:8080 returned a server error (503)."
        );
    }

    #[test]
    fn describe_digs_the_message_out_of_the_body() {
        let url = "https://api.github.com/repos/a/b/pulls/1/reviews";
        assert_eq!(
            describe(422, url, r#"{"message":"Validation Failed"}"#),
            "Validation Failed"
        );
        assert_eq!(
            describe(400, url, r#"{"error":"bad_request"}"#),
            "bad_request"
        );
        assert_eq!(
            describe(400, url, r#"{"error_description":"Token was revoked"}"#),
            "Token was revoked"
        );
        assert_eq!(
            describe(400, url, r#"{"errors":["line_code is invalid", "other"]}"#),
            "line_code is invalid"
        );
        // `String(object)`.
        assert_eq!(
            describe(422, url, r#"{"errors":[{"field":"body"}]}"#),
            "[object Object]"
        );
        // The first field present wins even when it says nothing usable.
        assert_eq!(
            describe(400, url, r#"{"message":{"base":["x"]},"error":"ignored"}"#),
            "api.github.com returned 400."
        );
        // A null field is skipped, as `??` skips it.
        assert_eq!(
            describe(400, url, r#"{"message":null,"error":"used"}"#),
            "used"
        );
        assert_eq!(
            describe(400, url, r#"{"errors":[]}"#),
            "api.github.com returned 400."
        );
        assert_eq!(
            describe(400, url, "<html>nope</html>"),
            "api.github.com returned 400."
        );
        assert_eq!(describe(400, url, "null"), "api.github.com returned 400.");
        assert_eq!(describe(409, url, ""), "api.github.com returned 409.");
    }

    #[test]
    fn safe_host_falls_back_to_the_input() {
        assert_eq!(safe_host("https://h.test:8443/a"), "h.test:8443");
        assert_eq!(safe_host("not a url"), "not a url");
    }

    // --- encode_form ---

    #[test]
    fn encode_form_flattens_nested_objects_into_brackets() {
        let body = json!({
            "body": "Looks good & done?",
            "position": {
                "position_type": "text",
                "new_line": 3,
                "old_line": null,
                "line_range": {
                    "start": { "line_code": "abc_1_2", "type": "new" },
                },
            },
            "empty": {},
            "skipped": null,
        });
        assert_eq!(
            encode_form(&body),
            "body=Looks%20good%20%26%20done%3F\
             &position%5Bposition_type%5D=text\
             &position%5Bnew_line%5D=3\
             &position%5Bline_range%5D%5Bstart%5D%5Bline_code%5D=abc_1_2\
             &position%5Bline_range%5D%5Bstart%5D%5Btype%5D=new"
        );
    }

    #[test]
    fn encode_form_repeats_arrays_and_writes_scalars_as_javascript_would() {
        let body = json!({
            "labels": ["a b", 2, null, true],
            "flag": false,
            "ratio": 0.5,
            "unicode": "č ✓",
            "unreserved": "-_.!~*'()",
        });
        assert_eq!(
            encode_form(&body),
            "labels%5B%5D=a%20b&labels%5B%5D=2&labels%5B%5D=null&labels%5B%5D=true\
             &flag=false&ratio=0.5&unicode=%C4%8D%20%E2%9C%93&unreserved=-_.!~*'()"
        );
        assert_eq!(encode_form(&json!({})), "");
        assert_eq!(encode_form(&json!("not an object")), "");
    }

    #[test]
    fn js_number_lays_out_numbers_like_number_to_string() {
        let cases = [
            (json!(3), "3"),
            (json!(-7), "-7"),
            (json!(1.5), "1.5"),
            (json!(-0.25), "-0.25"),
            (json!(0.0), "0"),
            (json!(100.0), "100"),
            (json!(1e21), "1e+21"),
            (json!(1.5e-7), "1.5e-7"),
            (json!(0.000001), "0.000001"),
            (json!(123456789012.5), "123456789012.5"),
        ];
        for (value, expected) in cases {
            assert_eq!(js_string(&value), expected, "{value}");
        }
        assert_eq!(js_string(&json!([1, null, "x", [2, 3]])), "1,,x,2,3");
        assert_eq!(js_string(&json!({})), "[object Object]");
    }

    // --- request, json, text and paginate through the mock ---

    #[test]
    fn a_json_body_goes_out_with_its_content_type() {
        let http = Http::mock(|request| {
            assert_eq!(request.method, Method::Post);
            assert_eq!(request.header("content-type"), Some("application/json"));
            assert_eq!(request.header("private-token"), Some("t0k"));
            assert_eq!(request.json_body(), Some(json!({ "body": "hi" })));
            MockResponse::json(201, &json!({ "id": 9 }))
        });
        let created: Value = block_on(
            http.json(
                "https://gitlab.test/api/v4/notes",
                RequestOptions::new(Method::Post)
                    .header("PRIVATE-TOKEN", "t0k")
                    .header("content-type", "text/plain")
                    .json(json!({ "body": "hi" })),
            ),
        )
        .unwrap();
        assert_eq!(created, json!({ "id": 9 }));
    }

    #[test]
    fn a_form_body_goes_out_encoded() {
        let http = Http::mock(|request| {
            assert_eq!(
                request.header("Content-Type"),
                Some("application/x-www-form-urlencoded")
            );
            assert_eq!(
                request.body.as_deref(),
                Some("position%5Bnew_line%5D=3&body=x")
            );
            MockResponse::new(204, "")
        });
        let nothing: Option<Value> = block_on(
            http.json(
                "https://gitlab.test/discussions",
                RequestOptions::new(Method::Post)
                    .form(json!({ "position": { "new_line": 3 }, "body": "x" })),
            ),
        )
        .unwrap();
        assert_eq!(nothing, None);
    }

    #[test]
    fn no_body_means_no_content_type() {
        let http = Http::mock(|request| {
            assert_eq!(request.header("content-type"), None);
            assert_eq!(request.body, None);
            MockResponse::new(200, "")
        });
        let () = block_on(http.json("https://h.test/x", RequestOptions::default())).unwrap();
    }

    #[test]
    fn json_reads_an_empty_body_as_null_and_text_as_a_string() {
        let http = Http::mock(|request| match request.url.as_str() {
            "https://h.test/empty" => MockResponse::new(200, ""),
            "https://h.test/bom" => MockResponse::new(200, b"\xEF\xBB\xBF{\"a\":1}".to_vec()),
            _ => MockResponse::new(200, "plain words"),
        });
        let empty: Option<Value> =
            block_on(http.json("https://h.test/empty", RequestOptions::default())).unwrap();
        assert_eq!(empty, None);
        let bom: Value =
            block_on(http.json("https://h.test/bom", RequestOptions::default())).unwrap();
        assert_eq!(bom, json!({ "a": 1 }));
        let words: String =
            block_on(http.json("https://h.test/words", RequestOptions::default())).unwrap();
        assert_eq!(words, "plain words");
        let words: Value =
            block_on(http.json("https://h.test/words", RequestOptions::default())).unwrap();
        assert_eq!(words, json!("plain words"));
    }

    #[test]
    fn json_of_the_wrong_shape_is_an_error_the_user_can_read() {
        #[derive(serde::Deserialize, Debug)]
        #[allow(dead_code)]
        struct Shape {
            id: u64,
        }
        let http = Http::mock(|_| MockResponse::json(200, &json!({ "id": "nine" })));
        let error = block_on(http.json::<Shape>("https://h.test/x", RequestOptions::default()))
            .unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("Unexpected response from h.test: "),
            "{error}"
        );
        let http = Http::mock(|_| MockResponse::new(204, ""));
        assert!(
            block_on(http.json::<Shape>("https://h.test/x", RequestOptions::default())).is_err()
        );
    }

    #[test]
    fn text_hands_back_the_body_whatever_it_is() {
        let http = Http::mock(|_| MockResponse::new(200, "diff --git a/x b/x\n"));
        assert_eq!(
            block_on(http.text("https://h.test/diff", RequestOptions::default())).unwrap(),
            "diff --git a/x b/x\n"
        );
    }

    #[test]
    fn a_non_2xx_response_becomes_an_api_error_with_the_described_message() {
        let http = Http::mock(|_| MockResponse::json(404, &json!({ "message": "404 Not Found" })));
        let error = block_on(http.request(
            "https://gitlab.test/api/v4/projects/1",
            RequestOptions::default(),
        ))
        .unwrap_err();
        match error {
            Error::Api {
                status,
                url,
                message,
                body,
            } => {
                assert_eq!(status, 404);
                assert_eq!(url, "https://gitlab.test/api/v4/projects/1");
                assert_eq!(message, "Not found on gitlab.test (/api/v4/projects/1).");
                assert_eq!(body.as_deref(), Some(r#"{"message":"404 Not Found"}"#));
            }
            other => panic!("expected an API error, got {other:?}"),
        }
    }

    #[test]
    fn send_hands_back_any_status() {
        let http = Http::mock(|request| {
            assert_eq!(request.method, Method::Get);
            MockResponse::new(302, "").header("Location", "https://cdn.test/img.png")
        });
        let response = block_on(http.send(
            "https://h.test/uploads/img.png",
            RequestOptions::default().max_redirects(0),
        ))
        .unwrap();
        assert_eq!(response.status, 302);
        assert!(!response.is_ok());
        assert_eq!(
            response.header("location").as_deref(),
            Some("https://cdn.test/img.png")
        );
    }

    #[test]
    fn transport_failures_read_as_the_typescript_wrote_them() {
        let http = Http::mock(|request| {
            if request.url.contains("slow") {
                MockResponse::timed_out()
            } else {
                MockResponse::unreachable("Connection refused (os error 61)")
            }
        });
        let error =
            block_on(http.request("https://slow.test/x", RequestOptions::default())).unwrap_err();
        assert_eq!(error.to_string(), "Request to slow.test timed out.");
        let error =
            block_on(http.request("https://down.test/x", RequestOptions::default())).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Could not reach down.test: Connection refused (os error 61)"
        );
        assert!(matches!(error, Error::Api { status: 0, .. }));
    }

    #[test]
    fn paginate_follows_next_links_with_the_same_headers() {
        let http = Http::mock(|request| {
            assert_eq!(request.method, Method::Get);
            assert_eq!(request.header("authorization"), Some("Bearer t"));
            assert_eq!(request.body, None);
            match request.url.as_str() {
                "https://api.test/items?page=1" => MockResponse::json(200, &json!([1, 2]))
                    .header("Link", r#"<https://api.test/items?page=2>; rel="next""#),
                "https://api.test/items?page=2" => MockResponse::json(200, &json!([3]))
                    .header("link", r#"<https://api.test/items?page=3>; rel="next", <https://api.test/items?page=3>; rel="last""#),
                "https://api.test/items?page=3" => MockResponse::json(200, &json!([4, 5])),
                other => panic!("unexpected page {other}"),
            }
        });
        let options = RequestOptions::new(Method::Post)
            .header("Authorization", "Bearer t")
            .json(json!({ "ignored": true }));
        let all: Vec<u32> =
            block_on(http.paginate("https://api.test/items?page=1", options.clone(), 5)).unwrap();
        assert_eq!(all, [1, 2, 3, 4, 5]);
        let capped: Vec<u32> =
            block_on(http.paginate("https://api.test/items?page=1", options, 2)).unwrap();
        assert_eq!(capped, [1, 2, 3]);
    }

    #[test]
    fn paginate_skips_a_page_that_is_not_an_array_and_fails_on_an_error_page() {
        let http = Http::mock(|request| match request.url.as_str() {
            "https://api.test/a" => MockResponse::json(200, &json!({ "message": "not a list" }))
                .header("link", "<https://api.test/b>; rel=\"next\""),
            "https://api.test/b" => MockResponse::json(200, &json!([7]))
                .header("link", "<https://api.test/c>; rel=\"next\""),
            _ => MockResponse::new(500, ""),
        });
        let error =
            block_on(http.paginate::<u32>("https://api.test/a", RequestOptions::default(), 5))
                .unwrap_err();
        match error {
            Error::Api { status, url, .. } => {
                assert_eq!(status, 500);
                // The page that failed, not the first one.
                assert_eq!(url, "https://api.test/c");
            }
            other => panic!("expected an API error, got {other:?}"),
        }
        let first_two: Vec<u32> =
            block_on(http.paginate("https://api.test/a", RequestOptions::default(), 2)).unwrap();
        assert_eq!(first_two, [7]);
    }

    #[test]
    fn paginate_rejects_entries_of_the_wrong_shape_and_bodies_that_are_not_json() {
        let http = Http::mock(|request| match request.url.as_str() {
            "https://api.test/shape" => MockResponse::json(200, &json!(["x"])),
            _ => MockResponse::new(200, "<html>"),
        });
        assert!(
            block_on(http.paginate::<u32>("https://api.test/shape", RequestOptions::default(), 5))
                .is_err()
        );
        assert!(
            block_on(http.paginate::<u32>("https://api.test/html", RequestOptions::default(), 5))
                .is_err()
        );
        assert_eq!(
            block_on(http.paginate::<u32>("https://api.test/shape", RequestOptions::default(), 0))
                .unwrap(),
            Vec::<u32>::new()
        );
    }

    // --- the real transport, against a server on the loopback interface ---

    mod real {
        use super::*;
        use std::io::{Read, Write};
        use std::net::{TcpListener, TcpStream};
        use std::thread;

        /// A tiny HTTP/1.1 server answering by path, one thread per connection.
        fn serve() -> String {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind the loopback");
            let address = listener.local_addr().expect("local address");
            thread::spawn(move || {
                for stream in listener.incoming().flatten() {
                    thread::spawn(move || answer(stream));
                }
            });
            format!("http://{address}")
        }

        fn answer(mut stream: TcpStream) {
            let mut request = Vec::new();
            let mut buffer = [0u8; 4096];
            let head_end = loop {
                let Ok(read) = stream.read(&mut buffer) else {
                    return;
                };
                if read == 0 {
                    return;
                }
                request.extend_from_slice(&buffer[..read]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let head = String::from_utf8_lossy(&request[..head_end]).to_string();
            let length: usize = head
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().ok())
                        .flatten()
                })
                .unwrap_or(0);
            while request.len() < head_end + length {
                let Ok(read) = stream.read(&mut buffer) else {
                    return;
                };
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
            }
            let body = String::from_utf8_lossy(&request[head_end..]).to_string();
            let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
            let agent = head
                .lines()
                .find(|line| line.to_ascii_lowercase().starts_with("user-agent:"))
                .map(|line| line[11..].trim().to_string())
                .unwrap_or_default();

            let (status, extra, payload) = match path.as_str() {
                "/json" => ("200 OK", String::new(), format!(r#"{{"agent":"{agent}"}}"#)),
                "/echo" => ("200 OK", String::new(), body),
                "/missing" => (
                    "404 Not Found",
                    String::new(),
                    r#"{"message":"gone"}"#.to_string(),
                ),
                "/redirect" => (
                    "302 Found",
                    "Location: /json\r\n".to_string(),
                    String::new(),
                ),
                "/slow" => {
                    thread::sleep(Duration::from_secs(3));
                    ("200 OK", String::new(), "late".to_string())
                }
                _ => ("500 Internal Server Error", String::new(), String::new()),
            };
            let response = format!(
                "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\nLink: <{path}?page=2>; rel=\"prev\"\r\n\r\n{payload}",
                payload.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }

        #[test]
        fn requests_run_on_the_private_runtime_and_are_awaited_from_any_executor() {
            let base = serve();
            let http = Http::new();

            let got: Value = block_on(http.json(
                &format!("{base}/json"),
                RequestOptions::default().header("User-Agent", "Reviewdeck"),
            ))
            .unwrap();
            assert_eq!(got, json!({ "agent": "Reviewdeck" }));

            let echoed = block_on(http.text(
                &format!("{base}/echo"),
                RequestOptions::new(Method::Put).json(json!({ "a": [1, 2] })),
            ))
            .unwrap();
            assert_eq!(echoed, r#"{"a":[1,2]}"#);

            let host = base.trim_start_matches("http://");
            let error =
                block_on(http.request(&format!("{base}/missing"), RequestOptions::default()))
                    .unwrap_err();
            assert_eq!(
                error.to_string(),
                format!("Not found on {host} (/missing).")
            );
        }

        #[test]
        fn redirects_are_followed_unless_asked_not_to() {
            let base = serve();
            let http = Http::new();

            let followed: Value =
                block_on(http.json(&format!("{base}/redirect"), RequestOptions::default()))
                    .unwrap();
            assert_eq!(followed, json!({ "agent": "" }));

            let held = block_on(http.send(
                &format!("{base}/redirect"),
                RequestOptions::default().max_redirects(0),
            ))
            .unwrap();
            assert_eq!(held.status, 302);
            assert_eq!(held.header("location").as_deref(), Some("/json"));
        }

        #[test]
        fn a_slow_host_times_out_and_a_closed_port_is_unreachable() {
            let base = serve();
            let http = Http::new();
            let host = base.trim_start_matches("http://");

            let error = block_on(http.request(
                &format!("{base}/slow"),
                RequestOptions::default().timeout(Duration::from_millis(200)),
            ))
            .unwrap_err();
            assert_eq!(error.to_string(), format!("Request to {host} timed out."));

            // Bind and drop a listener so the port is known to be closed.
            let closed = TcpListener::bind("127.0.0.1:0")
                .unwrap()
                .local_addr()
                .unwrap();
            let error =
                block_on(http.request(&format!("http://{closed}/x"), RequestOptions::default()))
                    .unwrap_err();
            assert!(
                error
                    .to_string()
                    .starts_with(&format!("Could not reach {closed}: ")),
                "{error}"
            );
        }
    }
}
