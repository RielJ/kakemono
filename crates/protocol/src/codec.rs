//! Newline-delimited JSON codec (plan §I) with a 1 MiB line cap, plus a JSON encoder that
//! matches Go's `encoding/json` byte for byte for the protocol types.

use std::io;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::ser::{CharEscape, Formatter};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Longest accepted line, excluding the trailing `\n`.
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error(transparent)]
    Io(#[from] io::Error),
    /// The line is longer than [`MAX_LINE_BYTES`]. The reader is left mid-line: close the
    /// connection.
    #[error("line exceeds {MAX_LINE_BYTES} bytes")]
    LineTooLong,
    /// The line was read completely but is not valid JSON for the target type. The reader is
    /// positioned at the next line, so the connection can keep going (Go answers
    /// `invalid JSON: ...` and continues).
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

/// Reads one line into `buf` (cleared first), without the trailing `\n` or `\r\n`, as Go's
/// `bufio.ScanLines` does. A last line with no `\n` before EOF still counts.
///
/// Returns `Ok(false)` at a clean EOF (nothing read).
pub async fn read_line<R>(reader: &mut R, buf: &mut Vec<u8>) -> Result<bool, CodecError>
where
    R: AsyncBufRead + Unpin + ?Sized,
{
    buf.clear();
    let limit = u64::try_from(MAX_LINE_BYTES)
        .unwrap_or(u64::MAX)
        .saturating_add(1);
    let n = (&mut *reader).take(limit).read_until(b'\n', buf).await?;
    if n == 0 {
        return Ok(false);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
        if buf.last() == Some(&b'\r') {
            buf.pop();
        }
    } else if buf.len() > MAX_LINE_BYTES {
        return Err(CodecError::LineTooLong);
    }
    Ok(true)
}

/// Reads one line and parses it as `T`. `Ok(None)` at a clean EOF.
pub async fn read_line_json<T, R>(reader: &mut R) -> Result<Option<T>, CodecError>
where
    T: DeserializeOwned,
    R: AsyncBufRead + Unpin + ?Sized,
{
    let mut buf = Vec::new();
    if !read_line(reader, &mut buf).await? {
        return Ok(None);
    }
    Ok(Some(serde_json::from_slice(&buf)?))
}

/// Writes `value` as one Go-compatible JSON line ([`to_vec_go`] plus `\n`) and flushes.
pub async fn write_line_json<T, W>(writer: &mut W, value: &T) -> Result<(), CodecError>
where
    T: Serialize + ?Sized,
    W: AsyncWrite + Unpin + ?Sized,
{
    let mut line = to_vec_go(value)?;
    line.push(b'\n');
    writer.write_all(&line).await?;
    writer.flush().await?;
    Ok(())
}

/// Serialises like Go's `json.Marshal`: compact, and `<`, `>`, `&`, U+2028 and U+2029
/// escaped as `\u003c`, `\u003e`, `\u0026`, `\u2028` and `\u2029`.
///
/// Matches Go for strings, integers, bools, null, and structs/maps (keys of
/// `serde_json::Map` are sorted, as Go sorts map keys). Floats are not covered: no protocol
/// type has one.
pub fn to_vec_go<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(128);
    let mut ser = serde_json::Serializer::with_formatter(&mut out, GoFormatter);
    value.serialize(&mut ser)?;
    Ok(out)
}

/// [`to_vec_go`] as a `String`.
pub fn to_string_go<T: Serialize + ?Sized>(value: &T) -> serde_json::Result<String> {
    let bytes = to_vec_go(value)?;
    // serde_json only writes valid UTF-8, and GoFormatter only adds ASCII.
    String::from_utf8(bytes).map_err(|e| serde::ser::Error::custom(e.to_string()))
}

struct GoFormatter;

impl Formatter for GoFormatter {
    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        let mut start = 0;
        for (i, c) in fragment.char_indices() {
            let esc: &[u8] = match c {
                '<' => b"\\u003c",
                '>' => b"\\u003e",
                '&' => b"\\u0026",
                '\u{2028}' => b"\\u2028",
                '\u{2029}' => b"\\u2029",
                _ => continue,
            };
            writer.write_all(&fragment.as_bytes()[start..i])?;
            writer.write_all(esc)?;
            start = i + c.len_utf8();
        }
        writer.write_all(&fragment.as_bytes()[start..])
    }

    fn write_char_escape<W>(&mut self, writer: &mut W, char_escape: CharEscape) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        // Same as serde_json's default, which already matches Go 1.22+ (`\b`, `\f`,
        // lowercase `\u00xx`). Spelled out so a serde_json change cannot drift silently.
        let s: &[u8] = match char_escape {
            CharEscape::Quote => b"\\\"",
            CharEscape::ReverseSolidus => b"\\\\",
            CharEscape::Solidus => b"\\/",
            CharEscape::Backspace => b"\\b",
            CharEscape::FormFeed => b"\\f",
            CharEscape::LineFeed => b"\\n",
            CharEscape::CarriageReturn => b"\\r",
            CharEscape::Tab => b"\\t",
            CharEscape::AsciiControl(byte) => {
                const HEX: &[u8; 16] = b"0123456789abcdef";
                let buf = [
                    b'\\',
                    b'u',
                    b'0',
                    b'0',
                    HEX[usize::from(byte >> 4)],
                    HEX[usize::from(byte & 0xF)],
                ];
                return writer.write_all(&buf);
            }
        };
        writer.write_all(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Command, Response};
    use tokio::io::BufReader;

    async fn lines(input: &[u8]) -> Vec<Result<Vec<u8>, String>> {
        let mut r = BufReader::new(input);
        let mut out = Vec::new();
        let mut buf = Vec::new();
        loop {
            match read_line(&mut r, &mut buf).await {
                Ok(true) => out.push(Ok(buf.clone())),
                Ok(false) => break,
                Err(e) => {
                    out.push(Err(e.to_string()));
                    break;
                }
            }
        }
        out
    }

    #[tokio::test]
    async fn splits_like_go_scanlines() {
        let got = lines(b"a\r\n\nb\nlast").await;
        let want: Vec<Result<Vec<u8>, String>> = vec![
            Ok(b"a".to_vec()),
            Ok(Vec::new()),
            Ok(b"b".to_vec()),
            Ok(b"last".to_vec()),
        ];
        assert_eq!(got, want);
    }

    #[tokio::test]
    async fn line_at_cap_is_accepted_and_one_over_is_rejected() {
        let mut ok = vec![b'x'; MAX_LINE_BYTES];
        ok.push(b'\n');
        ok.extend_from_slice(b"next\n");
        let got = lines(&ok).await;
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].as_ref().map(Vec::len), Ok(MAX_LINE_BYTES));
        assert_eq!(got[1], Ok(b"next".to_vec()));

        let mut over = vec![b'x'; MAX_LINE_BYTES + 1];
        over.push(b'\n');
        let got = lines(&over).await;
        assert_eq!(
            got,
            vec![Err(format!("line exceeds {MAX_LINE_BYTES} bytes"))]
        );
    }

    #[tokio::test]
    async fn bad_json_line_does_not_desync_the_stream() {
        let mut r = BufReader::new(&b"{not json\n\n{\"cmd\":\"status\"}\n"[..]);
        let e = read_line_json::<Command, _>(&mut r).await;
        assert!(matches!(e, Err(CodecError::Json(_))));
        let e = read_line_json::<Command, _>(&mut r).await;
        assert!(
            matches!(e, Err(CodecError::Json(_))),
            "empty line is invalid JSON"
        );
        let c = read_line_json::<Command, _>(&mut r).await.expect("ok");
        assert_eq!(c, Some(Command::new("status")));
        let eof = read_line_json::<Command, _>(&mut r).await.expect("eof");
        assert_eq!(eof, None);
    }

    #[tokio::test]
    async fn write_line_json_appends_newline() {
        let mut out = Vec::new();
        write_line_json(&mut out, &Response::ok())
            .await
            .expect("write");
        assert_eq!(out, b"{\"ok\":true}\n");
    }

    #[test]
    fn go_escaping() {
        let s = to_string_go("a<b>&c\u{2028}\u{2029}\"\\/\n\r\t\u{8}\u{c}\u{1}\u{1f}\u{7f}é")
            .expect("ser");
        assert_eq!(
            s,
            r#""a\u003cb\u003e\u0026c\u2028\u2029\"\\/\n\r\t\b\f\u0001\u001f"#.to_owned()
                + "\u{7f}é\""
        );
    }
}
