//! Go-compatible text: error messages and timestamps the Go daemon produced, which the wire
//! and the UI still show.

use std::io;
use std::path::Path;

use chrono::{DateTime, FixedOffset, SecondsFormat};
use kakemono_sources::http as source_http;
use reqwest::Url;

/// `message` with every spelling of `url` replaced by its redacted form (secret query values →
/// `REDACTED`, see `kakemono_sources::http::redact_url`). An unparsable URL loses its whole
/// query instead.
pub fn redact_in(message: &str, url: &str) -> String {
    match Url::parse(url) {
        Ok(parsed) => {
            let redacted = source_http::redact_url(&parsed);
            if redacted == parsed {
                return message.to_owned();
            }
            message
                .replace(url, redacted.as_str())
                .replace(parsed.as_str(), redacted.as_str())
        }
        Err(_) => match url.split_once('?') {
            Some((base, _)) => message.replace(url, &format!("{base}?{}", source_http::REDACTED)),
            None => message.to_owned(),
        },
    }
}

/// Go's `os.Stat` error text: `stat <path>: <reason>`, with Go's lower-case reasons for the
/// common cases.
pub(crate) fn go_stat_error(path: &Path, err: &io::Error) -> String {
    format!("stat {}: {}", path.display(), go_reason(err))
}

/// Go's lower-case `syscall.Errno` text for the common cases, else Rust's message.
pub(crate) fn go_reason(err: &io::Error) -> String {
    match err.kind() {
        io::ErrorKind::NotFound => "no such file or directory".to_owned(),
        io::ErrorKind::PermissionDenied => "permission denied".to_owned(),
        io::ErrorKind::NotADirectory => "not a directory".to_owned(),
        io::ErrorKind::IsADirectory => "is a directory".to_owned(),
        io::ErrorKind::AlreadyExists => "file exists".to_owned(),
        io::ErrorKind::CrossesDevices => "cross-device link".to_owned(),
        _ => err.to_string(),
    }
}

/// Go `time.Now().Format(time.RFC3339)`: whole seconds, `Z` for UTC.
pub(crate) fn format_set_at(now: &DateTime<FixedOffset>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Secs, true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_at_is_rfc3339_seconds() {
        let t = DateTime::parse_from_rfc3339("2026-01-02T03:04:05.678+08:00").unwrap();
        assert_eq!(format_set_at(&t), "2026-01-02T03:04:05+08:00");
        let utc = DateTime::parse_from_rfc3339("2026-01-02T03:04:05.9Z").unwrap();
        assert_eq!(format_set_at(&utc), "2026-01-02T03:04:05Z");
    }

    #[test]
    fn redact_in_replaces_raw_and_normalised_spellings() {
        let url = "https://cdn.test/a b.jpg?sig=S3CRET&w=1";
        let msg = format!(
            "download: GET {url}: error for url (https://cdn.test/a%20b.jpg?sig=S3CRET&w=1)"
        );
        let out = redact_in(&msg, url);
        assert!(!out.contains("S3CRET"), "{out}");
        assert!(out.contains("sig=REDACTED"), "{out}");
    }

    #[test]
    fn redact_in_unparsable_url_drops_query() {
        let out = redact_in(
            "download: invalid url ::bad?sig=S3CRET: x",
            "::bad?sig=S3CRET",
        );
        assert_eq!(out, "download: invalid url ::bad?REDACTED: x");
    }

    #[test]
    fn stat_error_text_matches_go() {
        let e = io::Error::from(io::ErrorKind::NotFound);
        assert_eq!(
            go_stat_error(Path::new("/x.png"), &e),
            "stat /x.png: no such file or directory"
        );
    }
}
