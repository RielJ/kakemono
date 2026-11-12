//! SSRF guard for URLs that come from provider responses (previews, full images, page links).
//!
//! Three layers, all needed:
//!
//! 1. [`check_url`] / [`GuardConfig::check_url`] before a request: `https` only, no
//!    credentials, no explicit port but 443, no IP literal, no `localhost`, and the host must
//!    match the provider's [`crate::Provider::media_hosts`] suffix list at a label boundary.
//! 2. [`BlockPrivateResolver`] at connect time: a listed name that resolves to a loopback,
//!    private, link-local, CGNAT, multicast, unspecified, ULA or v4-mapped address is refused
//!    (DNS rebinding, or a CDN name pointed at an internal host).
//! 3. [`guarded_client`]'s redirect policy: at most [`MAX_REDIRECTS`] hops, and every hop
//!    passes layer 1 again.
//!
//! The client does not check the first URL of a request (reqwest has no hook for it), so
//! callers run [`GuardConfig::check_url`] first. The resolver still applies to that URL.

use std::error::Error as StdError;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use reqwest::Client;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::redirect::Policy;
use url::{Host, Url};

use crate::http::{CONNECT_TIMEOUT, READ_TIMEOUT, USER_AGENT, redact_url};

/// The most redirect hops [`guarded_client`] follows. The fourth hop is an error.
pub const MAX_REDIRECTS: usize = 3;

/// Which checks the guard runs. [`GuardConfig::strict`] is the only mode for real use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GuardConfig {
    allow_loopback: bool,
}

impl GuardConfig {
    /// Every check on. The daemon always uses this.
    #[must_use]
    pub const fn strict() -> Self {
        Self {
            allow_loopback: false,
        }
    }

    /// Strict, except that `http`, any port, loopback IP literals, `localhost` and loopback
    /// DNS answers are allowed, so tests can reach wiremock on `127.0.0.1`. The host
    /// allowlist and the credential check still apply.
    #[doc(hidden)]
    #[must_use]
    pub const fn permissive_loopback() -> Self {
        Self {
            allow_loopback: true,
        }
    }

    /// Whether a connection to `ip` is allowed: never for [`is_blocked_ip`] addresses, except
    /// loopback in [`GuardConfig::permissive_loopback`].
    #[must_use]
    pub fn allows_ip(&self, ip: IpAddr) -> bool {
        !is_blocked_ip(ip) || (self.allow_loopback && ip.is_loopback())
    }

    /// Checks `url` before a request. `allowed_hosts` are host suffixes such as
    /// `"wallhaven.cc"`: `w.wallhaven.cc` matches, `evilwallhaven.cc` does not. An empty list
    /// allows nothing.
    ///
    /// # Errors
    ///
    /// The first failed check as a [`GuardError`], whose text shows the URL redacted.
    pub fn check_url<S: AsRef<str>>(
        &self,
        url: &Url,
        allowed_hosts: &[S],
    ) -> Result<(), GuardError> {
        let host = self.checked_host(url)?;
        if allowed_hosts
            .iter()
            .any(|suffix| host_matches(&host, suffix.as_ref()))
        {
            Ok(())
        } else {
            Err(GuardError::UnlistedHost {
                host,
                url: display_url(url),
            })
        }
    }

    /// Every check of [`GuardConfig::check_url`] except the host allowlist: scheme,
    /// credentials, port, IP literal and `localhost`. Returns the host as the allowlist
    /// matches it.
    fn checked_host(&self, url: &Url) -> Result<String, GuardError> {
        let shown = || display_url(url);
        match url.scheme() {
            "https" => {}
            "http" if self.allow_loopback => {}
            _ => return Err(GuardError::Scheme { url: shown() }),
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(GuardError::Credentials { url: shown() });
        }
        if !self.allow_loopback && url.port().is_some_and(|p| p != 443) {
            return Err(GuardError::Port { url: shown() });
        }
        let host = match url.host() {
            None => return Err(GuardError::NoHost { url: shown() }),
            Some(Host::Ipv4(ip)) => self.check_literal(IpAddr::V4(ip), url)?,
            Some(Host::Ipv6(ip)) => self.check_literal(IpAddr::V6(ip), url)?,
            Some(Host::Domain(name)) => {
                let name = name.strip_suffix('.').unwrap_or(name);
                if !self.allow_loopback && is_localhost(name) {
                    return Err(GuardError::Localhost { url: shown() });
                }
                name.to_owned()
            }
        };
        Ok(host)
    }

    /// The literal's text when it is a loopback address in permissive mode; an error
    /// otherwise.
    fn check_literal(&self, ip: IpAddr, url: &Url) -> Result<String, GuardError> {
        if self.allow_loopback && ip.is_loopback() {
            // `Url::host_str` keeps the brackets of an IPv6 literal; match on the bare text.
            Ok(ip.to_string())
        } else {
            Err(GuardError::IpLiteral {
                url: display_url(url),
            })
        }
    }
}

/// [`GuardConfig::strict`]'s [`GuardConfig::check_url`].
///
/// # Errors
///
/// See [`GuardConfig::check_url`].
pub fn check_url<S: AsRef<str>>(url: &Url, allowed_hosts: &[S]) -> Result<(), GuardError> {
    GuardConfig::strict().check_url(url, allowed_hosts)
}

/// Why the guard refused a URL, a redirect or a DNS answer. Every `url` field is already
/// redacted: secret query values replaced and credentials removed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum GuardError {
    /// The scheme is not `https`.
    #[error("blocked {url}: only https is allowed")]
    Scheme {
        /// The redacted URL.
        url: String,
    },
    /// The URL carries a user name or password.
    #[error("blocked {url}: credentials in URL")]
    Credentials {
        /// The redacted URL, credentials removed.
        url: String,
    },
    /// The URL names a port other than 443.
    #[error("blocked {url}: only port 443 is allowed")]
    Port {
        /// The redacted URL.
        url: String,
    },
    /// The host is an IP address instead of a name.
    #[error("blocked {url}: IP address hosts are not allowed")]
    IpLiteral {
        /// The redacted URL.
        url: String,
    },
    /// The host is `localhost` or a name under it.
    #[error("blocked {url}: localhost is not allowed")]
    Localhost {
        /// The redacted URL.
        url: String,
    },
    /// The URL has no host.
    #[error("blocked {url}: no host")]
    NoHost {
        /// The redacted URL.
        url: String,
    },
    /// The host matches none of the provider's media hosts.
    #[error("blocked {url}: host {host} is not on the allowlist")]
    UnlistedHost {
        /// The host that failed the match.
        host: String,
        /// The redacted URL.
        url: String,
    },
    /// A redirect beyond [`MAX_REDIRECTS`] hops.
    #[error("blocked {url}: more than {MAX_REDIRECTS} redirects")]
    TooManyRedirects {
        /// The redacted URL of the hop that was refused.
        url: String,
    },
    /// Every address the host resolved to is blocked (see [`is_blocked_ip`]).
    #[error("blocked {host}: resolves only to private or reserved addresses")]
    BlockedAddress {
        /// The host name that was looked up.
        host: String,
    },
}

impl GuardError {
    /// The first [`GuardError`] in `err` or its source chain. A guarded client's request
    /// error wraps the guard's refusal this way.
    #[must_use]
    pub fn find_in<'a>(err: &'a (dyn StdError + 'static)) -> Option<&'a Self> {
        let mut current = Some(err);
        while let Some(e) = current {
            if let Some(found) = e.downcast_ref::<Self>() {
                return Some(found);
            }
            current = e.source();
        }
        None
    }
}

/// `url` for error text: secret query values replaced ([`redact_url`]) and credentials
/// removed.
fn display_url(url: &Url) -> String {
    let mut shown = redact_url(url);
    // Both setters fail only for URLs that cannot have credentials, which then have none.
    let _ = shown.set_username("");
    let _ = shown.set_password(None);
    shown.to_string()
}

/// Whether `name` is `localhost` or under it (RFC 6761 reserves the whole tree).
fn is_localhost(name: &str) -> bool {
    host_matches(name, "localhost")
}

/// Whether `host` equals `suffix` or ends with `.{suffix}`, ignoring ASCII case. An empty
/// suffix matches nothing.
fn host_matches(host: &str, suffix: &str) -> bool {
    let suffix = suffix.trim_start_matches('.');
    if suffix.is_empty() || host.len() < suffix.len() {
        return false;
    }
    let (head, tail) = host.split_at(host.len() - suffix.len());
    tail.eq_ignore_ascii_case(suffix) && (head.is_empty() || head.ends_with('.'))
}

/// Whether a connection to `ip` must be refused: loopback, private, link-local, CGNAT,
/// multicast, broadcast, unspecified, reserved, documentation, ULA, IPv4-mapped and
/// IPv4-compatible IPv6, and NAT64 or 6to4 addresses that embed a blocked IPv4 address.
#[must_use]
pub fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_blocked_v4(v4),
        IpAddr::V6(v6) => is_blocked_v6(v6),
    }
}

fn is_blocked_v4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    ip.is_unspecified()
        || a == 0 // 0.0.0.0/8 "this network"
        || ip.is_loopback() // 127/8
        || ip.is_private() // 10/8, 172.16/12, 192.168/16
        || ip.is_link_local() // 169.254/16
        || (a == 100 && (b & 0xc0) == 64) // 100.64/10 CGNAT
        || (a == 192 && b == 0 && c == 0) // 192.0.0/24 IETF protocol assignments
        || ip.is_documentation() // 192.0.2/24, 198.51.100/24, 203.0.113/24
        || (a == 198 && (b & 0xfe) == 18) // 198.18/15 benchmarking
        || ip.is_multicast() // 224/4
        || a >= 240 // 240/4 reserved, and 255.255.255.255 broadcast
}

fn is_blocked_v6(ip: Ipv6Addr) -> bool {
    let s = ip.segments();
    // ::/96 (unspecified, loopback, IPv4-compatible) and ::ffff:0:0/96 (IPv4-mapped): a public
    // name has no business resolving to either, and the OS may route them to IPv4 hosts.
    let first80_zero = s[..5].iter().all(|&x| x == 0);
    if first80_zero && (s[5] == 0 || s[5] == 0xffff) {
        return true;
    }
    // 64:ff9b::/96 NAT64: blocked when the embedded IPv4 address is.
    if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        return is_blocked_v4(embedded_v4(s[6], s[7]));
    }
    // 2002::/16 6to4: the IPv4 address is in segments 1 and 2.
    if s[0] == 0x2002 {
        return is_blocked_v4(embedded_v4(s[1], s[2]));
    }
    (s[0] & 0xfe00) == 0xfc00 // fc00::/7 ULA
        || (s[0] & 0xffc0) == 0xfe80 // fe80::/10 link-local
        || (s[0] & 0xffc0) == 0xfec0 // fec0::/10 deprecated site-local
        || (s[0] & 0xff00) == 0xff00 // ff00::/8 multicast
        || (s[0] == 0x2001 && s[1] == 0x0db8) // 2001:db8::/32 documentation
        || (s[0] == 0x2001 && s[1] == 0) // 2001::/32 Teredo
        || s[..4] == [0x100, 0, 0, 0] // 100::/64 discard
}

fn embedded_v4(high: u16, low: u16) -> Ipv4Addr {
    let [a, b] = high.to_be_bytes();
    let [c, d] = low.to_be_bytes();
    Ipv4Addr::new(a, b, c, d)
}

/// A [`Resolve`] that looks names up with the system resolver and drops every address
/// [`GuardConfig::allows_ip`] refuses. A name left with no address fails with
/// [`GuardError::BlockedAddress`].
#[derive(Debug, Clone, Copy, Default)]
pub struct BlockPrivateResolver {
    config: GuardConfig,
}

impl BlockPrivateResolver {
    /// A resolver applying `config`'s address rules.
    #[must_use]
    pub const fn new(config: GuardConfig) -> Self {
        Self { config }
    }
}

impl Resolve for BlockPrivateResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let config = self.config;
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let found = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let allowed = filter_addrs(config, found);
            if allowed.is_empty() {
                tracing::warn!(%host, "guard: name resolves only to blocked addresses");
                return Err(Box::new(GuardError::BlockedAddress { host }) as Box<_>);
            }
            let addrs: Addrs = Box::new(allowed.into_iter());
            Ok(addrs)
        })
    }
}

/// The addresses in `addrs` that `config` allows, in order.
fn filter_addrs(
    config: GuardConfig,
    addrs: impl IntoIterator<Item = SocketAddr>,
) -> Vec<SocketAddr> {
    addrs
        .into_iter()
        .filter(|a| config.allows_ip(a.ip()))
        .collect()
}

/// A client for provider-supplied URLs: [`BlockPrivateResolver`], a redirect policy that
/// re-checks every hop against `redirect_hosts` and stops after [`MAX_REDIRECTS`], no proxy
/// (a proxy would resolve names itself and bypass the resolver), no `Referer`, and the
/// timeouts of [`crate::http::client`].
///
/// One client serves every provider, so `redirect_hosts` is the union of their
/// [`crate::Provider::media_hosts`]. The first URL of a request is not checked here; run
/// [`GuardConfig::check_url`] with the provider's own list first.
///
/// # Errors
///
/// When reqwest cannot set up its TLS backend.
pub fn guarded_client<S: AsRef<str>>(
    config: GuardConfig,
    redirect_hosts: &[S],
) -> Result<Client, reqwest::Error> {
    let hosts: Arc<[String]> = redirect_hosts
        .iter()
        .map(|h| h.as_ref().to_owned())
        .collect();
    client_checking(config, move |url| config.check_url(url, &hosts))
}

/// The client of [`guarded_client`], whose redirect policy runs `check` on every hop.
fn client_checking(
    config: GuardConfig,
    check: impl Fn(&Url) -> Result<(), GuardError> + Send + Sync + 'static,
) -> Result<Client, reqwest::Error> {
    let policy = Policy::custom(move |attempt| {
        // `previous` holds the original URL plus every hop already followed, so its length is
        // the number of this hop.
        if attempt.previous().len() > MAX_REDIRECTS {
            let url = display_url(attempt.url());
            tracing::warn!(%url, "guard: too many redirects");
            return attempt.error(GuardError::TooManyRedirects { url });
        }
        match check(attempt.url()) {
            Ok(()) => attempt.follow(),
            Err(err) => {
                tracing::warn!(error = %err, "guard: redirect refused");
                attempt.error(err)
            }
        }
    });
    Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT)
        .https_only(!config.allow_loopback)
        .no_proxy()
        .referer(false)
        .redirect(policy)
        .dns_resolver(BlockPrivateResolver::new(config))
        .build()
}

/// The most hosts one [`GuardedHttp`] keeps; extra hosts are dropped.
pub const MAX_ROW_HOSTS: usize = 64;

/// A [`guarded_client`] plus the fixed host list it was built for: the first URL
/// ([`GuardedHttp::check_url`]) and every redirect hop are checked against the same list.
/// One per source whose hosts are not known at compile time, so two such sources never
/// share an allowlist. Cheap to clone.
///
/// [`GuardedHttp::any_public_host`] is the one variant without a list, for a link the user
/// typed.
#[derive(Clone)]
pub struct GuardedHttp {
    config: GuardConfig,
    hosts: Arc<[String]>,
    /// No host allowlist: [`GuardedHttp::any_public_host`].
    any_host: bool,
    client: Client,
}

impl GuardedHttp {
    /// Builds the client for `hosts`, deduped (case-insensitive, first spelling kept) and cut
    /// to the first [`MAX_ROW_HOSTS`].
    ///
    /// # Errors
    ///
    /// When reqwest cannot set up its TLS backend.
    pub fn new<S: AsRef<str>>(config: GuardConfig, hosts: &[S]) -> Result<Self, reqwest::Error> {
        let mut kept: Vec<String> = Vec::new();
        for host in hosts.iter().map(AsRef::as_ref) {
            if kept.iter().any(|k| k.eq_ignore_ascii_case(host)) {
                continue;
            }
            if kept.len() == MAX_ROW_HOSTS {
                tracing::debug!(%host, "guard: host list full, host dropped");
                continue;
            }
            kept.push(host.to_owned());
        }
        let client = guarded_client(config, &kept)?;
        Ok(Self {
            config,
            hosts: kept.into(),
            any_host: false,
            client,
        })
    }

    /// A client for a link the user typed, such as a web page row's location: every
    /// [`GuardConfig::check_url`] check except the host allowlist (scheme, credentials, port,
    /// IP literal, `localhost`), on the first URL and on every redirect hop, so the page may
    /// redirect to another public host as in a browser. [`BlockPrivateResolver`],
    /// [`MAX_REDIRECTS`], no proxy and the timeouts still apply.
    ///
    /// Only for links the user typed, never for URLs taken from a response: those go through
    /// [`GuardedHttp::new`] with a fixed list. [`GuardedHttp::hosts`] is empty.
    ///
    /// # Errors
    ///
    /// When reqwest cannot set up its TLS backend.
    pub fn any_public_host(config: GuardConfig) -> Result<Self, reqwest::Error> {
        let client = client_checking(config, move |url| config.checked_host(url).map(drop))?;
        Ok(Self {
            config,
            hosts: Arc::from([]),
            any_host: true,
            client,
        })
    }

    /// [`GuardConfig::check_url`] against this list (without the list for
    /// [`GuardedHttp::any_public_host`]). Call it before the first request of every URL; the
    /// client checks only redirects.
    ///
    /// # Errors
    ///
    /// See [`GuardConfig::check_url`].
    pub fn check_url(&self, url: &Url) -> Result<(), GuardError> {
        if self.any_host {
            self.config.checked_host(url).map(drop)
        } else {
            self.config.check_url(url, &self.hosts)
        }
    }

    /// The guarded client. Its redirect policy uses [`GuardedHttp::hosts`].
    #[must_use]
    pub fn client(&self) -> &Client {
        &self.client
    }

    /// The host suffixes this client allows, deduped and capped. Empty for
    /// [`GuardedHttp::any_public_host`], which checks no list.
    #[must_use]
    pub fn hosts(&self) -> &[String] {
        &self.hosts
    }

    /// The checks this client runs.
    #[must_use]
    pub fn config(&self) -> GuardConfig {
        self.config
    }
}

impl std::fmt::Debug for GuardedHttp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GuardedHttp")
            .field("config", &self.config)
            .field("hosts", &self.hosts)
            .field("any_host", &self.any_host)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sock(s: &str) -> SocketAddr {
        SocketAddr::new(s.parse().unwrap(), 443)
    }

    #[test]
    fn filter_drops_private_and_keeps_public() {
        let got = filter_addrs(
            GuardConfig::strict(),
            [
                sock("10.0.0.1"),
                sock("8.8.8.8"),
                sock("::1"),
                sock("2606:4700::1111"),
            ],
        );
        assert_eq!(got, vec![sock("8.8.8.8"), sock("2606:4700::1111")]);
    }

    #[test]
    fn filter_permissive_keeps_loopback_only() {
        let got = filter_addrs(
            GuardConfig::permissive_loopback(),
            [sock("127.0.0.1"), sock("192.168.0.1"), sock("::1")],
        );
        assert_eq!(got, vec![sock("127.0.0.1"), sock("::1")]);
    }

    #[test]
    fn host_matches_is_case_insensitive() {
        assert!(host_matches("W.WallHaven.CC", "wallhaven.cc"));
    }

    #[test]
    fn host_matches_ignores_leading_dot_in_suffix() {
        assert!(host_matches("w.wallhaven.cc", ".wallhaven.cc"));
    }

    #[test]
    fn host_matches_rejects_shorter_host() {
        assert!(!host_matches("cc", "wallhaven.cc"));
    }

    #[test]
    fn blocks_6to4_with_private_v4() {
        assert!(is_blocked_ip("2002:0a00:0001::1".parse().unwrap()));
    }

    #[test]
    fn allows_6to4_with_public_v4() {
        assert!(!is_blocked_ip("2002:0808:0808::1".parse().unwrap()));
    }
}
