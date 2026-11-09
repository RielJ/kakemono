//! `set <path>`: set a local image (Go `cmd/set.go`). The path is made absolute here, because
//! the daemon runs with a different working directory.

use std::io::Write;
use std::path::{Component, Path, PathBuf};

use anyhow::Context as _;
use kakemono_protocol::types::Request;

use crate::cli::Context;
use crate::output;

pub async fn run(ctx: &Context, out: &mut dyn Write, path: &Path) -> anyhow::Result<()> {
    let home = kakemono_config::defaults::home_dir();
    let cwd = std::env::current_dir().context("resolving path")?;
    let path = absolute_path(path, &home, &cwd);
    let response = ctx
        .call(Request::Set {
            path: path.to_string_lossy().into_owned(),
        })
        .await?;
    output::write_info_response(out, &response)
}

/// Go `expandPath`: `~/…` → home, then `filepath.Abs` (join with `cwd`, then clean `.` and `..`
/// lexically). Symlinks are not resolved, and the file need not exist (the daemon reports
/// `file not found`).
pub fn absolute_path(path: &Path, home: &Path, cwd: &Path) -> PathBuf {
    let expanded = match path.strip_prefix("~") {
        Ok(rest) if !home.as_os_str().is_empty() && path != Path::new("~") => home.join(rest),
        _ => path.to_owned(),
    };
    let joined = if expanded.is_absolute() {
        expanded
    } else {
        cwd.join(expanded)
    };
    clean(&joined)
}

/// Lexical cleanup like Go `filepath.Clean` for absolute paths: drops `.`, resolves `..`
/// against the previous component, never above the root.
fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if matches!(out.components().next_back(), Some(Component::Normal(_))) {
                    out.pop();
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn makes_paths_absolute_like_go() {
        let home = Path::new("/home/u");
        let cwd = Path::new("/work/dir");
        let cases = [
            ("a.jpg", "/work/dir/a.jpg"),
            ("./a.jpg", "/work/dir/a.jpg"),
            ("../x/a.jpg", "/work/x/a.jpg"),
            ("/abs/./b/../a.jpg", "/abs/a.jpg"),
            ("~/Pictures/a.jpg", "/home/u/Pictures/a.jpg"),
            ("~", "/work/dir/~"),
            ("~other/a.jpg", "/work/dir/~other/a.jpg"),
            ("/../../a.jpg", "/a.jpg"),
        ];
        for (input, want) in cases {
            assert_eq!(
                absolute_path(Path::new(input), home, cwd),
                PathBuf::from(want),
                "{input}"
            );
        }
    }
}
