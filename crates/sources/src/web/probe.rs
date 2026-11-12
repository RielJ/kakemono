//! Image sizes read from the first bytes of each candidate, a few at a time, within a time
//! budget.

use std::sync::Arc;
use std::time::Duration;

use reqwest::header::{CONTENT_TYPE, RANGE};
use reqwest::{Client, Response, StatusCode};
use tokio::sync::Semaphore;
use tokio::task::JoinSet;
use tokio::time::Instant;
use url::Url;

use super::PROBE_MAX_BYTES;
use super::dimensions::dimensions;
use crate::http::redact_url;

/// Probes in flight at once.
pub(crate) const PROBE_CONCURRENCY: usize = 4;

/// The `Range` asked for. A server may ignore it; the read stops at [`PROBE_MAX_BYTES`] anyway.
const PROBE_RANGE: &str = "bytes=0-65535";

/// The size of each of `urls`, in the same order: `None` when the request failed, the format
/// is not JPEG, PNG or WebP, the size was not in the first [`PROBE_MAX_BYTES`] bytes, the probe
/// took longer than `timeout`, or `deadline` passed first.
///
/// `client` checks redirects only: the caller checks each URL first.
pub(crate) async fn probe_all(
    client: &Client,
    urls: &[Url],
    timeout: Duration,
    deadline: Instant,
) -> Vec<Option<(u32, u32)>> {
    let mut sizes = vec![None; urls.len()];
    let permits = Arc::new(Semaphore::new(PROBE_CONCURRENCY));
    let mut tasks = JoinSet::new();
    for (index, url) in urls.iter().enumerate() {
        let client = client.clone();
        let url = url.clone();
        let permits = Arc::clone(&permits);
        tasks.spawn(async move {
            // The semaphore is never closed, so `acquire` cannot fail.
            let _permit = permits.acquire().await.ok();
            let size = tokio::time::timeout(timeout, probe(&client, &url))
                .await
                .ok()
                .flatten();
            (index, size)
        });
    }
    let collect = async {
        while let Some(joined) = tasks.join_next().await {
            if let Ok((index, size)) = joined
                && let Some(slot) = sizes.get_mut(index)
            {
                *slot = size;
            }
        }
    };
    if tokio::time::timeout_at(deadline, collect).await.is_err() {
        tracing::debug!("web: probe budget ran out");
    }
    // Dropping `tasks` aborts the probes still running.
    sizes
}

/// The size of the image at `url` from its first bytes. A response that is not `200`/`206`, or
/// whose `Content-Type` says it is not an image, is dropped before its body is read.
async fn probe(client: &Client, url: &Url) -> Option<(u32, u32)> {
    let response = client
        .get(url.clone())
        .header(RANGE, PROBE_RANGE)
        .send()
        .await
        .ok()?;
    if !matches!(
        response.status(),
        StatusCode::OK | StatusCode::PARTIAL_CONTENT
    ) {
        return None;
    }
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .map(|v| v.to_str().unwrap_or_default());
    if !may_be_image(content_type) {
        tracing::debug!(url = %redact_url(url), ?content_type, "web: probe skipped, not an image");
        return None;
    }
    let prefix = read_prefix(response, PROBE_MAX_BYTES, |buf| dimensions(buf).is_some())
        .await
        .ok()?;
    dimensions(&prefix)
}

/// Whether a response with this `Content-Type` may hold an image: `image/*`, or no type, an
/// empty one or a generic binary one, which say nothing (the bytes decide).
fn may_be_image(content_type: Option<&str>) -> bool {
    let Some(value) = content_type else {
        return true;
    };
    let mime = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    mime.is_empty()
        || mime.starts_with("image/")
        || mime == "application/octet-stream"
        || mime == "binary/octet-stream"
}

/// The body of `response`, read chunk by chunk until `done` holds for what was read, the body
/// ends, or `cap` bytes were read (the rest is not read; the response is dropped).
pub(crate) async fn read_prefix(
    mut response: Response,
    cap: usize,
    mut done: impl FnMut(&[u8]) -> bool,
) -> Result<Vec<u8>, reqwest::Error> {
    let mut buf = Vec::new();
    while buf.len() < cap {
        let Some(chunk) = response.chunk().await? else {
            break;
        };
        let room = cap - buf.len();
        buf.extend_from_slice(chunk.get(..room).unwrap_or(&chunk));
        if done(&buf) {
            break;
        }
    }
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::may_be_image;

    #[test]
    fn may_be_image_reads_images_and_unknown_types_only() {
        let types = [
            None,
            Some(""),
            Some("image/jpeg"),
            Some("Image/PNG; charset=binary"),
            Some("application/octet-stream"),
            Some("binary/octet-stream"),
            Some("text/html; charset=UTF-8"),
            Some("application/json"),
        ];
        let read: Vec<bool> = types.into_iter().map(may_be_image).collect();
        assert_eq!(read, [true, true, true, true, true, true, false, false]);
    }
}
