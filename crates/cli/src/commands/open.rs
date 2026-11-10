//! `open [--folder | --favorites]` (Go `cmd/open.go`): reveal the current wallpaper in the file
//! manager, or open the download or favorites folder.

use std::io::Write;

use anyhow::Context as _;
use kakemono_config::paths::Paths;
use kakemono_platform::open::Opener;

use crate::cli::Context;

/// What `open` shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The current wallpaper file, selected (the default).
    Current,
    /// `[daemon] download_dir` (`--folder`).
    DownloadDir,
    /// `[favorites] dir` (`--favorites`).
    Favorites,
}

impl Target {
    /// From the flags. clap rejects both at once; `--favorites` wins if both get here, as in Go.
    pub fn from_flags(folder: bool, favorites: bool) -> Self {
        if favorites {
            Target::Favorites
        } else if folder {
            Target::DownloadDir
        } else {
            Target::Current
        }
    }
}

pub async fn run(
    ctx: &Context,
    out: &mut dyn Write,
    folder: bool,
    favorites: bool,
) -> anyhow::Result<()> {
    run_with(
        ctx,
        out,
        Target::from_flags(folder, favorites),
        &Opener::system(),
    )
    .await
}

/// [`run`] with an explicit opener (a recording one in tests).
pub async fn run_with(
    ctx: &Context,
    out: &mut dyn Write,
    target: Target,
    opener: &Opener,
) -> anyhow::Result<()> {
    if target == Target::Current {
        let path = ctx
            .status()
            .await?
            .current
            .map(|c| c.path)
            .filter(|p| !p.is_empty())
            .context("no wallpaper currently set")?;
        writeln!(out, "Revealing: {path}")?;
        out.flush()?;
        opener.reveal(path.as_ref()).await?;
        return Ok(());
    }

    let paths = Paths::detect().context("loading config")?;
    let (cfg, _) = paths
        .load_config(ctx.global.config.as_deref())
        .context("loading config")?;
    let dir = match target {
        Target::Favorites => cfg.favorites.dir,
        _ => cfg.daemon.download_dir,
    };
    writeln!(out, "Opening: {}", dir.display())?;
    out.flush()?;
    opener.open_folder(&dir).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use kakemono_platform::exec::RecordingRunner;
    use kakemono_platform::open::{NoFileManager, Os};
    use kakemono_protocol::endpoint::Endpoint;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    use super::*;
    use crate::cli::GlobalArgs;

    fn recording(os: Os) -> (Opener, RecordingRunner) {
        let runner = RecordingRunner::new();
        let opener = Opener::with_parts(os, Arc::new(runner.clone()), Arc::new(NoFileManager));
        (opener, runner)
    }

    fn ctx(config: Option<&Path>, socket: Option<&Path>) -> Context {
        Context::new(GlobalArgs {
            config: config.map(Path::to_path_buf),
            socket: socket.map(Path::to_path_buf),
        })
    }

    /// A fake daemon on `socket` that answers one request with `reply` and returns the request.
    fn serve_once(socket: &Path, reply: String) -> tokio::task::JoinHandle<String> {
        let listener = Endpoint::new(socket).unwrap().bind().unwrap();
        tokio::spawn(async move {
            use interprocess::local_socket::traits::tokio::Listener as _;
            let stream = listener.accept().await.unwrap();
            let mut line = String::new();
            BufReader::new(&stream).read_line(&mut line).await.unwrap();
            let mut tx = &stream;
            tx.write_all(reply.as_bytes()).await.unwrap();
            tx.write_all(b"\n").await.unwrap();
            line
        })
    }

    #[test]
    fn flags() {
        assert_eq!(Target::from_flags(false, false), Target::Current);
        assert_eq!(Target::from_flags(true, false), Target::DownloadDir);
        assert_eq!(Target::from_flags(false, true), Target::Favorites);
    }

    #[tokio::test]
    async fn folder_and_favorites_open_the_configured_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let downloads = tmp.path().join("dl dir");
        let favorites = tmp.path().join("favs");
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::create_dir_all(&favorites).unwrap();
        let config = tmp.path().join("config.toml");
        let toml = format!(
            "[daemon]\ndownload_dir = {:?}\n\n[favorites]\ndir = {:?}\n",
            downloads.to_str().unwrap(),
            favorites.to_str().unwrap()
        );
        std::fs::write(&config, toml).unwrap();
        let ctx = ctx(Some(&config), None);

        let (opener, runner) = recording(Os::MacOs);
        let mut out = Vec::new();
        run_with(&ctx, &mut out, Target::DownloadDir, &opener)
            .await
            .unwrap();
        run_with(&ctx, &mut out, Target::Favorites, &opener)
            .await
            .unwrap();
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!(
                "Opening: {}\nOpening: {}\n",
                downloads.display(),
                favorites.display()
            )
        );
        assert_eq!(
            runner.argvs(),
            [
                vec!["open".to_owned(), downloads.to_str().unwrap().to_owned()],
                vec!["open".to_owned(), favorites.to_str().unwrap().to_owned()],
            ]
        );
    }

    #[tokio::test]
    async fn missing_folder_is_an_error_after_the_message() {
        let tmp = tempfile::tempdir().unwrap();
        let downloads = tmp.path().join("not-created");
        let config = tmp.path().join("config.toml");
        std::fs::write(
            &config,
            format!(
                "[daemon]\ndownload_dir = {:?}\n",
                downloads.to_str().unwrap()
            ),
        )
        .unwrap();
        let (opener, runner) = recording(Os::Linux);
        let mut out = Vec::new();
        let err = run_with(
            &ctx(Some(&config), None),
            &mut out,
            Target::DownloadDir,
            &opener,
        )
        .await
        .unwrap_err();
        assert_eq!(
            format!("{err:#}"),
            format!("no such file or directory: {}", downloads.display())
        );
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("Opening: {}\n", downloads.display())
        );
        assert!(runner.detached().is_empty() && runner.argvs().is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn reveals_the_current_wallpaper_from_status() {
        let tmp = tempfile::tempdir().unwrap();
        let image = tmp.path().join("wall paper.jpg");
        std::fs::write(&image, b"x").unwrap();
        let socket = tmp.path().join("k.sock");
        let reply = serde_json::json!({
            "ok": true,
            "data": {
                "current": { "path": image.to_str().unwrap() },
                "paused": false, "interval": "30m", "theme": "dark",
                "history_count": 1, "history_index": 0
            }
        })
        .to_string();
        let server = serve_once(&socket, reply);

        let (opener, runner) = recording(Os::MacOs);
        let mut out = Vec::new();
        run_with(
            &ctx(None, Some(&socket)),
            &mut out,
            Target::Current,
            &opener,
        )
        .await
        .unwrap();
        assert_eq!(server.await.unwrap().trim(), r#"{"cmd":"status"}"#);
        assert_eq!(
            String::from_utf8(out).unwrap(),
            format!("Revealing: {}\n", image.display())
        );
        assert_eq!(
            runner.argvs(),
            [vec![
                "open".to_owned(),
                "-R".to_owned(),
                image.to_str().unwrap().to_owned()
            ]]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn no_current_wallpaper() {
        let tmp = tempfile::tempdir().unwrap();
        let socket = tmp.path().join("k.sock");
        let reply = r#"{"ok":true,"data":{"current":null,"paused":false,"interval":"30m","theme":"light","history_count":0,"history_index":-1}}"#;
        let server = serve_once(&socket, reply.to_owned());
        let (opener, runner) = recording(Os::MacOs);
        let mut out = Vec::new();
        let err = run_with(
            &ctx(None, Some(&socket)),
            &mut out,
            Target::Current,
            &opener,
        )
        .await
        .unwrap_err();
        server.await.unwrap();
        assert_eq!(format!("{err:#}"), "no wallpaper currently set");
        assert!(out.is_empty());
        assert!(runner.argvs().is_empty());
    }
}
