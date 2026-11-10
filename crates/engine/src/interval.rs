//! A `[daemon]` interval setting with a fallback: the rotation and download loops parse theirs
//! the same way and warn once per bad value.

use std::fmt::Display;
use std::time::Duration;

/// Remembers the bad value already warned about, so a loop warns once per value.
#[derive(Debug, Default)]
pub(crate) struct IntervalSetting {
    warned: Option<String>,
}

impl IntervalSetting {
    /// `parsed` if it is a non-zero duration, else `default` with a warning naming `key` and
    /// `raw` (once per `raw` value).
    pub(crate) fn resolve<E: Display>(
        &mut self,
        key: &str,
        raw: &str,
        parsed: Result<Duration, E>,
        default: Duration,
    ) -> Duration {
        let problem = match parsed {
            Ok(d) if !d.is_zero() => {
                self.warned = None;
                return d;
            }
            Ok(_) => "the interval is zero".to_owned(),
            Err(e) => e.to_string(),
        };
        if self.warned.as_deref() != Some(raw) {
            tracing::warn!(
                interval = %raw,
                %problem,
                "invalid [daemon] {key}, defaulting to {}",
                minutes(default)
            );
            self.warned = Some(raw.to_owned());
        }
        default
    }

    /// The bad value last warned about.
    #[cfg(test)]
    pub(crate) fn warned(&self) -> Option<&str> {
        self.warned.as_deref()
    }
}

/// `30m`, `10m`: the defaults are whole minutes.
fn minutes(d: Duration) -> String {
    format!("{}m", d.as_secs() / 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_keeps_good_values_and_falls_back_on_bad_ones() {
        let mut s = IntervalSetting::default();
        let def = Duration::from_secs(600);
        let ok: Result<Duration, String> = Ok(Duration::from_secs(5));
        assert_eq!(s.resolve("interval", "5s", ok, def), Duration::from_secs(5));
        assert_eq!(s.warned(), None);
        let zero: Result<Duration, String> = Ok(Duration::ZERO);
        assert_eq!(s.resolve("interval", "0s", zero, def), def);
        assert_eq!(s.warned(), Some("0s"));
        let bad: Result<Duration, String> = Err("bad".to_owned());
        assert_eq!(s.resolve("interval", "nope", bad, def), def);
        assert_eq!(s.warned(), Some("nope"));
        let ok: Result<Duration, String> = Ok(Duration::from_secs(5));
        s.resolve("interval", "5s", ok, def);
        assert_eq!(s.warned(), None);
        assert_eq!(minutes(def), "10m");
    }
}
