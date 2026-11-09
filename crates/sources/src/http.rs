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

/// Total per-request timeout of [`client`].
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// Delay before retry 1, 2 and 3 when the response has no usable `Retry-After`.
/// A request is sent at most `1 + BACKOFF.len()` times.
pub const BACKOFF: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
];

/// Longest `Retry-After` we wait for. A longer one ends the retries and the 429/503 response is
/// returned to the caller, so a rotation tick never stalls for an hour.
pub const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// Query parameters whose values are secrets (API keys, client ids, tokens). [`redact_url`]
/// replaces their values; matching ignores ASCII case.
pub const SECRET_QUERY_PARAMS: [&str; 6] = [
    "apikey",
    "api_key",
    "client_id",
    "access_key",
    "key",
    "token",
];

/// What [`redact_url`] puts in place of a secret query value.
pub const REDACTED: &str = "REDACTED";

/// The shared client: [`USER_AGENT`] and a [`TIMEOUT`] total timeout.
///
/// Build it once and clone it (a `Client` is an `Arc`); providers take it in their constructor.
pub fn client() -> Result<Client, reqwest::Error> {
    client_builder().timeout(TIMEOUT).build()
}

/// A builder with [`USER_AGENT`] set and **no** timeout.
///
/// [`client`] adds the timeout. Use this directly only when you need a different setup, e.g.
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
/// - A `Retry-After` above [`MAX_RETRY_AFTER`] stops retrying.
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

/// Whether [`send_with_retry`] retries a response with this status.
pub fn is_retryable(status: StatusCode) -> bool {
    status == StatusCode::TOO_MANY_REQUESTS || status == StatusCode::SERVICE_UNAVAILABLE
}

/// The delay a `Retry-After` header asks for, measured from `now`.
///
/// Accepts delta-seconds (`"2"`) and an HTTP-date (`"Wed, 21 Oct 2015 07:28:00 GMT"`).
/// A date in the past gives `Duration::ZERO`. Missing or unparseable gives `None`.
pub fn retry_after(headers: &HeaderMap, now: SystemTime) -> Option<Duration> {
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
pub fn url_with_query(base: &str, params: &[(&str, &str)]) -> Result<url::Url, url::ParseError> {
    let mut url = url::Url::parse(base)?;
    let mut sorted = params.to_vec();
    sorted.sort_by_key(|&(k, _)| k);
    if !sorted.is_empty() {
        url.query_pairs_mut().extend_pairs(sorted);
    }
    Ok(url)
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
pub fn redact_error(err: reqwest::Error) -> reqwest::Error {
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
/// Body-read failures (URL redacted, see [`redact_error`]) become [`ProviderError::Request`] and
/// parse failures
/// [`ProviderError::Decode`], both prefixed with `context`. The status is not checked: do that
/// first, since providers word status errors differently.
pub async fn read_json<T: DeserializeOwned>(
    response: Response,
    context: &str,
) -> Result<T, ProviderError> {
    let body = response
        .bytes()
        .await
        .map_err(|e| ProviderError::request(context, redact_error(e)))?;
    serde_json::from_slice(&body).map_err(|e| ProviderError::decode(context, e))
}

/// Reads the body of `response` as text (for HTML scraping). Failures (URL redacted) become
/// [`ProviderError::Request`] prefixed with `context`.
pub async fn read_text(response: Response, context: &str) -> Result<String, ProviderError> {
    response
        .text()
        .await
        .map_err(|e| ProviderError::request(context, redact_error(e)))
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
        let port = {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap().port()
        };
        let url = format!("http://127.0.0.1:{port}/p?client_id=s3cret&page=1");
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
}
