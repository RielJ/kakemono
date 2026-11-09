//! Print every platform signal as it arrives. Never sets the wallpaper.
//!
//! ```sh
//! cargo run -p kakemono-platform --example signals            # until Ctrl-C
//! cargo run -p kakemono-platform --example signals -- --for 5 # exit after 5 s
//! ```
//!
//! On macOS: switch Space, sleep and wake, toggle Appearance, plug in a display; each should
//! print within about 1 s.

use std::time::Duration;

use kakemono_config::schema::DesktopConfig;

fn main() -> anyhow::Result<()> {
    let limit = parse_for()?;
    kakemono_platform::host(&DesktopConfig::default(), move |platform| async move {
        println!("backend: {:?}", platform.setter.info());
        println!("theme:   {}", platform.signals.theme());
        let mut rx = platform.signals.subscribe();
        let printing = async {
            loop {
                match rx.recv().await {
                    Ok(signal) => println!("signal:  {signal:?}"),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        println!("lagged:  {n} signals dropped");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        let deadline = async {
            match limit {
                Some(limit) => tokio::time::sleep(limit).await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            () = printing => {}
            () = deadline => {}
            res = tokio::signal::ctrl_c() => res?,
        }
        println!("bye");
        Ok(())
    })
}

/// `--for <seconds>`: stop after that long.
fn parse_for() -> anyhow::Result<Option<Duration>> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--for" {
            let secs: u64 = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("--for needs a number of seconds"))?
                .parse()?;
            return Ok(Some(Duration::from_secs(secs)));
        }
    }
    Ok(None)
}
