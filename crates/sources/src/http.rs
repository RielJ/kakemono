//! Shared HTTP client and the retry helper every provider and `download` use (plan §G "Retry").
//!
//! Retries are hand-rolled (reqwest-retry ignores `Retry-After`). Sleeps use `tokio::time`, so
//! tests can run under a paused clock.

use std::time::{Duration, SystemTime};

use reqwest::header::{HeaderMap, RETRY_AFTER};
use reqwest::{Client, ClientBuilder, IntoUrl, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;

use crate::provider::ProviderError;

/// `User-Agent` sent with every request: `kakemono/<crate version>`.
pub const USER_AGENT: &str = concat!("kakemono/", env!("CARGO_PKG_VERSION"));

/// Longest wait of [`client`] for a TCP (and TLS) connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Longest silence [`client`] accepts while waiting for the response head or the next body
/// chunk. The timer restarts with every chunk, so a large image on a slow link keeps going.
pub const READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Delay before retry 1, 2 and 3 when the response has no usable `Retry-After`.
/// A request is sent at most `1 + BACKOFF.len()` times.
pub(crate) const BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// Longest `Retry-After` we wait for. A longer one ends the retries and the 429/503 response is
/// returned to the caller, so a rotation tick never stalls for an hour.
pub(crate) const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Largest API (JSON) or scraped HTML body [`read_json`] and [`read_text`] accept: 8 MiB.
pub const MAX_API_BODY_BYTES: u64 = 8 * 1024 * 1024;

/// Query parameters whose values are secrets (API keys, client ids, tokens, and the signature
/// and credential parts of signed image URLs: generic `sig`/`signature`, AWS S3/CloudFront,
/// Google Cloud Storage). [`redact_url`] replaces their values; matching ignores ASCII case.
pub const SECRET_QUERY_PARAMS: [&str; 13] = [
    "apikey",
    "api_key",
    "client_id",
    "access_key",
    "key",
    "token",
    "sig",
    "signature",
    "x-amz-signature",
    "x-amz-credential",
    "x-amz-security-token",
    "x-goog-signature",
    "x-goog-credential",
];

/// What [`redact_url`] puts in place of a secret query value.
pub const REDACTED: &str = "REDACTED";

/// The shared client: [`USER_AGENT`], [`CONNECT_TIMEOUT`] and [`READ_TIMEOUT`].
///
/// There is no total deadline, because it would also cover image bodies and cut off a large
/// image on a slow link. Stalls are caught by the two timeouts above; response sizes are
/// capped by [`read_json`], [`read_text`] and [`crate::download()`].
///
/// Build it once and clone it (a `Client` is an `Arc`); providers take it in their constructor.
///
/// # Errors
///
/// When reqwest cannot set up its TLS backend.
pub fn client() -> Result<Client, reqwest::Error> {
    with_timeouts(client_builder(), CONNECT_TIMEOUT, READ_TIMEOUT).build()
}

/// `builder` with stall timeouts and no total deadline (see [`client`]).
fn with_timeouts(builder: ClientBuilder, connect: Duration, read: Duration) -> ClientBuilder {
    builder.connect_timeout(connect).read_timeout(read)
}

/// A builder with [`USER_AGENT`] set and **no** timeouts.
///
/// [`client`] adds the timeouts. Use this directly only when you need a different setup, e.g.
/// tests under `tokio::time::pause`, where a pending timeout timer would auto-advance the
/// paused clock and fire while a request is in flight.
pub fn client_builder() -> ClientBuilder {
    Client::builder().user_agent(USER_AGENT)
}

/// `GET url` with [`send_with_retry`].
pub async fn get_with_retry(
    client: &Client,
    url: impl IntoUrl,
) -> Result<Response, reqwest::Error> {
    send_with_retry(client.get(url)).await
}

/// Sends `request`, retrying on 429 Too Many Requests and 503 Service Unavailable.
///
/// - Up to `BACKOFF.len()` retries (4 sends in total).
/// - The delay is the response's `Retry-After` (delta-seconds or HTTP-date) when present and
///   parseable, otherwise `BACKOFF[retry]`.
/// - A `Retry-After` above 60 seconds stops retrying.
/// - When retries run out or stop, the last 429/503 response is returned as `Ok`, so the
///   provider maps the status to its own message (e.g. APOD's rate-limit text).
/// - Transport errors are returned at once, without retry, with their URL passed through
///   [`redact_url`].
/// - A request whose body cannot be cloned (a stream) is sent once.
pub async fn send_with_retry(request: RequestBuilder) -> Result<Response, reqwest::Error> {
    let mut retry = 0;
    loop {
        let Some(attempt) = request.try_clone() else {
            return request.send().await.map_err(redact_error);
        };
        let response = attempt.send().await.map_err(redact_error)?;
        if !is_retryable(response.status()) {
            return Ok(response);
        }
        let Some(backoff) = BACKOFF.get(retry) else {
            return Ok(response);
        };
        let delay = match retry_after(response.headers(), SystemTime::now()) {
            Some(d) if d > MAX_RETRY_AFTER => return Ok(response),
            Some(d) => d,
            None => *backoff,
        };
        tracing::debug!(
            url = %redact_url(response.url()),
            status = response.status().as_u16(),
            ?delay,
            retry = retry + 1,
            "retrying request"
        );
        drop(response);
        tokio::time::sleep(delay).await;
        retry += 1;
    }
}

/// Sends `request` once, for browse paths where the UI is waiting: a 429 or 503 is an error
/// at once, never a sleep.
///
/// - 429 Too Many Requests and 503 Service Unavailable become
///   [`ProviderError::RateLimitedFor`] with `{context}: rate limited (status {code})` and the
///   [`retry_after_secs`] hint. The message is the same with or without a hint.
/// - Transport errors become [`ProviderError::Request`], URL passed through [`redact_url`].
/// - Any other status is returned as `Ok`, so the provider still runs its status checks.
///
/// Fetch paths keep [`send_with_retry`]: the engine downloader's back-off matches only
/// [`ProviderError::RateLimited`].
///
/// # Errors
///
/// See above.
pub async fn send_fail_fast(
    request: RequestBuilder,
    context: &str,
) -> Result<Response, ProviderError> {
    let response = request
        .send()
        .await
        .map_err(|e| ProviderError::request(context, redact_error(e)))?;
    let status = response.status();
    if !is_retryable(status) {
        return Ok(response);
    }
    let retry_after_secs = retry_after_secs(response.headers());
    tracing::debug!(
        url = %redact_url(response.url()),
        status = status.as_u16(),
        ?retry_after_secs,
        "rate limited, not retrying"
    );
    Err(ProviderError::RateLimitedFor {
        message: format!("{context}: rate limited (status {})", status.as_u16()),
        retry_after_secs,
    })
}

/// Whole seconds a `Retry-After` header asks to wait, rounded up: delta-seconds (`"60"`) or
/// an HTTP-date (a past date gives 0). `None` when the header is missing or unparseable.
#[must_use]
pub fn retry_after_secs(headers: &HeaderMap) -> Option<u64> {
    retry_after_secs_at(headers, SystemTime::now())
}

/// [`retry_after_secs`] measured from `now`.
fn retry_after_secs_at(headers: &HeaderMap, now: SystemTime) -> Option<u64> {
    let delay = retry_after(headers, now)?;
    Some(delay.as_secs() + u64::from(delay.subsec_nanos() > 0))
}

/// A non-200 API status, classified once for every provider so a 429 means the same thing
/// everywhere. Providers turn it into their own error text, or use
/// [`BadStatus::into_provider_error`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BadStatus {
    /// 429 Too Many Requests, still there after the retries of [`send_with_retry`].
    RateLimited,
    /// 401 Unauthorized or 403 Forbidden: a missing, wrong or revoked key.
    Unauthorized(StatusCode),
    /// Any other status but 200.
    Other(StatusCode),
}

impl BadStatus {
    /// The default error: `RateLimited` becomes [`ProviderError::RateLimited`] with
    /// `{context}: rate limited (status 429)`; any other status [`ProviderError::Status`]
    /// (`{context}: unexpected status {code}`).
    pub(crate) fn into_provider_error(self, context: &str) -> ProviderError {
        match self {
            Self::RateLimited => rate_limited(context),
            Self::Unauthorized(status) | Self::Other(status) => {
                ProviderError::status(context, status)
            }
        }
    }
}

/// `{context}: rate limited (status 429)` as [`ProviderError::RateLimited`].
pub(crate) fn rate_limited(context: &str) -> ProviderError {
    ProviderError::RateLimited(format!(
        "{context}: rate limited (status {})",
        StatusCode::TOO_MANY_REQUESTS.as_u16()
    ))
}

/// `Ok(response)` when its status is 200 OK, otherwise the status classified as
/// [`BadStatus`].
pub(crate) fn check_status(response: Response) -> Result<Response, BadStatus> {
    match response.status() {
        StatusCode::OK => Ok(response),
        StatusCode::TOO_MANY_REQUESTS => Err(BadStatus::RateLimited),
        status @ (StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) => {
            Err(BadStatus::Unauthorized(status))
        }
        status => Err(BadStatus::Other(status)),
    }
}

/// Whether [`send_with_retry`] retries a response with this status.
pub(crate) fn is_retryable(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE
}

/// The delay a `Retry-After` header asks for, measured from `now`.
///
/// Accepts delta-seconds (`"2"`) and an HTTP-date (`"Wed, 21 Oct 2015 07:28:00 GMT"`).
/// A date in the past gives `Duration::ZERO`. Missing or unparseable gives `None`.
pub(crate) fn retry_after(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
    let value = headers.get(RETRY_AFTER)?.to_str().ok()?.trim();
    if let Ok(secs) = value.parse::<u64>() {
        return Some(Duration::from_secs(secs));
    }
    let at = httpdate::parse_http_date(value).ok()?;
    Some(at.duration_since(now).unwrap_or(Duration::ZERO))
}

/// `base` with `params` appended as a query string, keys sorted like Go's `url.Values.Encode`.
///
/// Use it for every query URL: reqwest's `RequestBuilder::query` is not enabled in this
/// workspace, and sorted keys reproduce the URLs inside Go error texts (e.g. wallhaven's
/// `unexpected status 500 for https://...?apikey=..&categories=..`). Pairs with the same key keep
/// their order. Spaces encode as `+`. Existing query pairs in `base` are kept, before `params`.
///
/// # Errors
///
/// When `base` is not an absolute URL.
pub fn url_with_query(base: &str, params: &[(&str, &str)]) -> Result<url::Url, url::ParseError> {
    let mut url = url::Url::parse(base)?;
    append_sorted_query(&mut url, params);
    Ok(url)
}

/// `params` appended to the query of `url`, keys sorted like Go's `url.Values.Encode`.
fn append_sorted_query(url: &mut url::Url, params: &[(&str, &str)]) {
    let mut sorted = params.to_vec();
    sorted.sort_by_key(|&(k, _)| k);
    if !sorted.is_empty() {
        url.query_pairs_mut().extend_pairs(sorted);
    }
}

/// The API endpoint `path` (starting with `/`) under a provider's `base` URL, with `params` as
/// its query (sorted, see [`url_with_query`]).
///
/// This is the one place that joins a base URL and a path: a trailing `/` on the base path is
/// dropped (`http://h/` and `http://h` give the same URL), and any query or fragment on the
/// base is dropped. Cannot fail, because `base` is already parsed.
pub fn endpoint(base: &url::Url, path: &str, params: &[(&str, &str)]) -> url::Url {
    let mut url = base.clone();
    url.set_path(&format!("{}{path}", base.path().trim_end_matches('/')));
    url.set_query(None);
    url.set_fragment(None);
    append_sorted_query(&mut url, params);
    url
}

/// Parses one of the crate's production `BASE_URL` constants.
///
/// The `default_base_urls_parse` test covers every constant, so this cannot panic.
#[allow(
    clippy::expect_used,
    reason = "only called with constants, which a test parses"
)]
pub(crate) fn default_base_url(constant: &'static str) -> url::Url {
    url::Url::parse(constant).expect("BASE_URL constants are absolute URLs")
}

/// `url` with the value of every [`SECRET_QUERY_PARAMS`] parameter replaced by [`REDACTED`].
/// Other parameters keep their order and values. A URL without a secret parameter is returned
/// unchanged; one with a secret has its query re-encoded (`form_urlencoded`, so a space shows
/// as `+`). For log lines and error texts only, never for requests.
pub fn redact_url(url: &url::Url) -> url::Url {
    let is_secret = |k: &str| {
        SECRET_QUERY_PARAMS
            .iter()
            .any(|s| s.eq_ignore_ascii_case(k))
    };
    if !url.query_pairs().any(|(k, _)| is_secret(&k)) {
        return url.clone();
    }
    let pairs: Vec<(String, String)> = url
        .query_pairs()
        .map(|(k, v)| {
            let v = if is_secret(&k) {
                REDACTED.to_owned()
            } else {
                v.into_owned()
            };
            (k.into_owned(), v)
        })
        .collect();
    let mut redacted = url.clone();
    redacted.query_pairs_mut().clear().extend_pairs(pairs);
    redacted
}

/// `err` with the URL its `Display` shows passed through [`redact_url`]. Errors without a URL,
/// or whose URL has no secret, are returned as is.
pub(crate) fn redact_error(err: reqwest::Error) -> reqwest::Error {
    let redacted = match err.url() {
        Some(url) => {
            let redacted = redact_url(url);
            if &redacted == url {
                return err;
            }
            redacted
        }
        None => return err,
    };
    err.with_url(redacted)
}

/// Reads the body of `response` and decodes it as JSON.
///
/// Body-read failures (secret query values redacted) become [`ProviderError::Request`],
/// a body over [`MAX_API_BODY_BYTES`] [`ProviderError::TooLarge`] and parse failures
/// [`ProviderError::Decode`], all prefixed with `context`. The status is not checked: callers
/// check it first.
///
/// # Errors
///
/// See above.
pub async fn read_json<T: DeserializeOwned>(
    response: Response,
    context: &str,
) -> Result<T, ProviderError> {
    let body = read_capped(response, context).await?;
    serde_json::from_slice(&body).map_err(|e| ProviderError::decode(context, e))
}

/// Reads the body of `response` as text (for HTML scraping), decoded as UTF-8 with invalid
/// sequences replaced.
///
/// # Errors
///
/// Body-read failures (URL redacted) become [`ProviderError::Request`] and a body over
/// [`MAX_API_BODY_BYTES`] [`ProviderError::TooLarge`], both prefixed with `context`.
pub async fn read_text(response: Response, context: &str) -> Result<String, ProviderError> {
    let body = read_capped(response, context).await?;
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// The whole body of `response`, at most [`MAX_API_BODY_BYTES`] long.
async fn read_capped(mut response: Response, context: &str) -> Result<Vec<u8>, ProviderError> {
    let too_large = || ProviderError::TooLarge {
        context: context.to_owned(),
        limit: MAX_API_BODY_BYTES,
    };
    if response
        .content_length()
        .is_some_and(|n| n > MAX_API_BODY_BYTES)
    {
        return Err(too_large());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| ProviderError::request(context, redact_error(e)))?
    {
        if (body.len() + chunk.len()) as u64 > MAX_API_BODY_BYTES {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    fn headers(value: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(RETRY_AFTER, HeaderValue::from_str(value).unwrap());
        h
    }

    #[test]
    fn retry_after_seconds() {
        let now = SystemTime::now();
        assert_eq!(
            retry_after(&headers("2"), now),
            Some(Duration::from_secs(2))
        );
        assert_eq!(retry_after(&headers(" 0 "), now), Some(Duration::ZERO));
    }

    #[test]
    fn retry_after_http_date() {
        let now = httpdate::parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT").unwrap();
        assert_eq!(
            retry_after(&headers("Wed, 21 Oct 2015 07:28:05 GMT"), now),
            Some(Duration::from_secs(5))
        );
        // A date in the past means "retry now".
        assert_eq!(
            retry_after(&headers("Wed, 21 Oct 2015 07:27:00 GMT"), now),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn retry_after_missing_or_garbage() {
        let now = SystemTime::now();
        assert_eq!(retry_after(&HeaderMap::new(), now), None);
        assert_eq!(retry_after(&headers("soon"), now), None);
        assert_eq!(retry_after(&headers("-1"), now), None);
    }

    #[test]
    fn retry_after_secs_delta_seconds() {
        assert_eq!(retry_after_secs(&headers("60")), Some(60));
    }

    #[test]
    fn retry_after_secs_http_date() {
        let now = httpdate::parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT").unwrap();
        assert_eq!(
            retry_after_secs_at(&headers("Wed, 21 Oct 2015 07:29:00 GMT"), now),
            Some(60)
        );
    }

    #[test]
    fn retry_after_secs_rounds_partial_seconds_up() {
        let now = httpdate::parse_http_date("Wed, 21 Oct 2015 07:28:00 GMT").unwrap()
            + Duration::from_millis(500);
        assert_eq!(
            retry_after_secs_at(&headers("Wed, 21 Oct 2015 07:28:05 GMT"), now),
            Some(5)
        );
    }

    #[test]
    fn retry_after_secs_past_date_is_zero() {
        assert_eq!(
            retry_after_secs(&headers("Wed, 21 Oct 2015 07:28:00 GMT")),
            Some(0)
        );
    }

    #[test]
    fn retry_after_secs_missing() {
        assert_eq!(retry_after_secs(&HeaderMap::new()), None);
    }

    #[test]
    fn retry_after_secs_garbage() {
        assert_eq!(retry_after_secs(&headers("soon")), None);
    }

    #[test]
    fn retryable_statuses() {
        assert!(is_retryable(StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable(StatusCode::SERVICE_UNAVAILABLE));
        assert!(!is_retryable(StatusCode::INTERNAL_SERVER_ERROR));
        assert!(!is_retryable(StatusCode::OK));
    }

    #[test]
    fn user_agent_is_kakemono_version() {
        assert_eq!(
            USER_AGENT,
            format!("kakemono/{}", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn url_with_query_sorts_keys_like_go() {
        let url = url_with_query(
            "https://wallhaven.cc/api/v1/search",
            &[
                ("sorting", "random"),
                ("apikey", "k"),
                ("q", "red sky"),
                ("categories", "111"),
            ],
        )
        .unwrap();
        assert_eq!(
            url.as_str(),
            "https://wallhaven.cc/api/v1/search?apikey=k&categories=111&q=red+sky&sorting=random"
        );
        let bare = url_with_query("https://x.test/a", &[]).unwrap();
        assert_eq!(bare.as_str(), "https://x.test/a");
    }

    #[test]
    fn default_base_urls_parse() {
        for constant in [
            crate::apod::BASE_URL,
            crate::bing::BASE_URL,
            crate::reddit::BASE_URL,
            crate::unsplash::BASE_URL,
            crate::wallhaven::DEFAULT_BASE_URL,
        ] {
            assert!(url::Url::parse(constant).is_ok(), "{constant}");
            assert_eq!(default_base_url(constant).path(), "/", "{constant}");
        }
    }

    #[test]
    fn endpoint_joins_base_and_path_one_way() {
        let params = [("n", "8"), ("format", "js")];
        let want = "http://h.test/HPImageArchive.aspx?format=js&n=8";
        for base in ["http://h.test", "http://h.test/", "http://h.test/?x=1#f"] {
            let base = url::Url::parse(base).unwrap();
            assert_eq!(
                endpoint(&base, "/HPImageArchive.aspx", &params).as_str(),
                want
            );
        }
        let prefixed = url::Url::parse("http://h.test/proxy/").unwrap();
        assert_eq!(
            endpoint(&prefixed, "/a", &[]).as_str(),
            "http://h.test/proxy/a"
        );
    }

    #[test]
    fn redact_url_blanks_every_secret_param() {
        let url = url::Url::parse(
            "https://api.unsplash.com/photos/random?client_id=s3cret&count=5&query=red+sky\
             &apikey=a&api_key=b&access_key=c&key=d&token=e&Client_ID=f",
        )
        .unwrap();
        let redacted = redact_url(&url);
        assert_eq!(
            redacted.as_str(),
            "https://api.unsplash.com/photos/random?client_id=REDACTED&count=5&query=red+sky\
             &apikey=REDACTED&api_key=REDACTED&access_key=REDACTED&key=REDACTED&token=REDACTED\
             &Client_ID=REDACTED"
        );
        assert!(!redacted.as_str().contains("s3cret"));
    }

    #[test]
    fn redact_url_blanks_signed_url_params() {
        let url = url::Url::parse(
            "https://cdn.test/i.jpg?w=10&sig=S1&Signature=S2&X-Amz-Signature=S3\
             &X-Amz-Credential=S4&X-Amz-Security-Token=S5&X-Goog-Signature=S6\
             &X-Goog-Credential=S7&X-Amz-Expires=60",
        )
        .unwrap();
        let redacted = redact_url(&url);
        assert_eq!(
            redacted.as_str(),
            "https://cdn.test/i.jpg?w=10&sig=REDACTED&Signature=REDACTED\
             &X-Amz-Signature=REDACTED&X-Amz-Credential=REDACTED\
             &X-Amz-Security-Token=REDACTED&X-Goog-Signature=REDACTED\
             &X-Goog-Credential=REDACTED&X-Amz-Expires=60"
        );
    }

    #[test]
    fn redact_url_without_secret_is_unchanged() {
        let url = url::Url::parse("https://x.test/a?q=a%20b&monkey=1&keys=2").unwrap();
        assert_eq!(redact_url(&url), url);
        assert_eq!(
            redact_url(&url).as_str(),
            "https://x.test/a?q=a%20b&monkey=1&keys=2"
        );
    }

    #[tokio::test]
    async fn transport_error_url_is_redacted() {
        // A server that closes every connection unanswered: a transport error, on a port this
        // test keeps bound (a freed port could be taken by another process meanwhile).
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                drop(socket);
            }
        });
        let url = format!("http://{addr}/p?client_id=s3cret&page=1");
        let err = get_with_retry(&client_builder().build().unwrap(), url)
            .await
            .unwrap_err();
        let text = format!("{err} {err:?}");
        assert!(!text.contains("s3cret"), "{text}");
        assert!(text.contains("client_id=REDACTED&page=1"), "{text}");
    }

    #[test]
    fn client_builds() {
        client().unwrap();
    }

    /// Read timeout for the stall tests: long enough for loopback, short enough to keep the
    /// tests fast.
    const TEST_READ_TIMEOUT: Duration = Duration::from_millis(400);

    /// A client with the production timeout setup, scaled down to [`TEST_READ_TIMEOUT`].
    fn test_client() -> Client {
        with_timeouts(client_builder(), Duration::from_secs(2), TEST_READ_TIMEOUT)
            .build()
            .unwrap()
    }

    /// One-shot HTTP server on a port it keeps bound. It answers 200 with a `Content-Length`
    /// of `declared` bytes, sends `chunks × chunk_len` bytes, sleeping `gap` before each chunk,
    /// then holds the connection open for `hold_after` without sending more.
    async fn trickle_server(
        declared: usize,
        chunks: usize,
        chunk_len: usize,
        gap: Duration,
        hold_after: Duration,
    ) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = socket.read(&mut buf).await.unwrap();
                if n == 0 {
                    return;
                }
                request.extend_from_slice(&buf[..n]);
            }
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: image/jpeg\r\nContent-Length: {declared}\r\n\r\n"
            );
            if socket.write_all(head.as_bytes()).await.is_err() {
                return;
            }
            for _ in 0..chunks {
                tokio::time::sleep(gap).await;
                if socket.write_all(&vec![0xAB; chunk_len]).await.is_err() {
                    return;
                }
            }
            tokio::time::sleep(hold_after).await;
        });
        format!("http://{addr}/img.jpg")
    }

    async fn read_body(client: &Client, url: &str) -> Result<usize, reqwest::Error> {
        let mut response = get_with_retry(client, url).await?;
        let mut total = 0;
        while let Some(chunk) = response.chunk().await? {
            total += chunk.len();
        }
        Ok(total)
    }

    /// A large image on a slow link keeps sending, just slowly. No total deadline may cut it
    /// off: the body here takes 3× the read timeout but never pauses for longer than a quarter
    /// of it.
    #[tokio::test]
    async fn slow_but_steady_body_is_not_cut_off() {
        let gap = TEST_READ_TIMEOUT / 4;
        let url = trickle_server(12 * 1024, 12, 1024, gap, Duration::ZERO).await;
        let got = tokio::time::timeout(Duration::from_secs(10), read_body(&test_client(), &url))
            .await
            .expect("test deadline");
        assert_eq!(got.unwrap(), 12 * 1024);
    }

    /// A server that stops sending mid-body is detected after the read timeout, not after the
    /// whole body would have arrived.
    #[tokio::test]
    async fn stalled_body_fails_after_read_timeout() {
        let url = trickle_server(2048, 1, 1024, Duration::ZERO, Duration::from_secs(30)).await;
        let mut response = get_with_retry(&test_client(), &url).await.unwrap();
        let started = tokio::time::Instant::now();
        let err = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                match response.chunk().await {
                    Ok(Some(_)) => {}
                    Ok(None) => panic!("body ended, but the server never sent all of it"),
                    Err(e) => return e,
                }
            }
        })
        .await
        .expect("test deadline");
        assert!(err.is_timeout(), "{err:?}");
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
    }
}
