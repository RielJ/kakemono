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
/// - Transport errors are returned at once, without retry.
/// - A request whose body cannot be cloned (a stream) is sent once.
pub async fn send_with_retry(request: RequestBuilder) -> Result<Response, reqwest::Error> {
    let mut retry = 0;
    loop {
        let Some(attempt) = request.try_clone() else {
            return request.send().await;
        };
        let response = attempt.send().await?;
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
            url = %response.url(),
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

/// Reads the body of `response` and decodes it as JSON.
///
/// Body-read failures become [`ProviderError::Request`] and parse failures
/// [`ProviderError::Decode`], both prefixed with `context`. The status is not checked: do that
/// first, since providers word status errors differently.
pub async fn read_json<T: DeserializeOwned>(
    response: Response,
    context: &str,
) -> Result<T, ProviderError> {
    let body = response
        .bytes()
        .await
        .map_err(|e| ProviderError::request(context, e))?;
    serde_json::from_slice(&body).map_err(|e| ProviderError::decode(context, e))
}

/// Reads the body of `response` as text (for HTML scraping). Failures become
/// [`ProviderError::Request`] prefixed with `context`.
pub async fn read_text(response: Response, context: &str) -> Result<String, ProviderError> {
    response
        .text()
        .await
        .map_err(|e| ProviderError::request(context, e))
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
    fn client_builds() {
        client().unwrap();
    }
}
