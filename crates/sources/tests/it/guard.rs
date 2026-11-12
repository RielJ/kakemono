//! SSRF guard: `check_url`, `is_blocked_ip`, the private-address resolver and the redirect
//! policy of `guarded_client`. Run: `cargo test -p kakemono-sources guard`.
//!
//! Every test name starts with `guard_` so that filter selects them.

use std::net::IpAddr;

use kakemono_sources::guard::{
    GuardConfig, GuardError, GuardedHttp, MAX_REDIRECTS, MAX_ROW_HOSTS, check_url, guarded_client,
    is_blocked_ip,
};
use url::Url;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const WALLHAVEN: &[&str] = &["wallhaven.cc"];

fn strict(url: &str) -> Result<(), GuardError> {
    check_url(&Url::parse(url).unwrap(), WALLHAVEN)
}

fn ip(s: &str) -> IpAddr {
    s.parse().unwrap()
}

#[test]
fn guard_accepts_listed_https_host() {
    assert!(strict("https://w.wallhaven.cc/full/x.jpg").is_ok());
}

#[test]
fn guard_accepts_exact_listed_host() {
    assert!(strict("https://wallhaven.cc/x.jpg").is_ok());
}

#[test]
fn guard_accepts_explicit_port_443() {
    assert!(strict("https://w.wallhaven.cc:443/x.jpg").is_ok());
}

#[test]
fn guard_accepts_trailing_dot_host() {
    assert!(strict("https://w.wallhaven.cc./x.jpg").is_ok());
}

#[test]
fn guard_rejects_http() {
    assert!(matches!(
        strict("http://w.wallhaven.cc/x.jpg"),
        Err(GuardError::Scheme { .. })
    ));
}

#[test]
fn guard_rejects_ipv4_literal() {
    assert!(matches!(
        strict("https://127.0.0.1/"),
        Err(GuardError::IpLiteral { .. })
    ));
}

#[test]
fn guard_rejects_ipv6_literal() {
    assert!(matches!(
        strict("https://[::1]/"),
        Err(GuardError::IpLiteral { .. })
    ));
}

#[test]
fn guard_rejects_obfuscated_ipv4_literal() {
    assert!(matches!(
        strict("https://0x7f.1/"),
        Err(GuardError::IpLiteral { .. })
    ));
}

#[test]
fn guard_rejects_localhost() {
    assert!(matches!(
        strict("https://localhost/"),
        Err(GuardError::Localhost { .. })
    ));
}

#[test]
fn guard_rejects_localhost_subdomain() {
    assert!(matches!(
        strict("https://w.localhost/"),
        Err(GuardError::Localhost { .. })
    ));
}

#[test]
fn guard_rejects_credentials() {
    assert!(matches!(
        strict("https://user:pw@w.wallhaven.cc/"),
        Err(GuardError::Credentials { .. })
    ));
}

#[test]
fn guard_rejects_username_only() {
    assert!(matches!(
        strict("https://user@w.wallhaven.cc/"),
        Err(GuardError::Credentials { .. })
    ));
}

#[test]
fn guard_credentials_error_hides_password() {
    let err = strict("https://user:hunter2@w.wallhaven.cc/").unwrap_err();
    assert!(!err.to_string().contains("hunter2"), "{err}");
}

#[test]
fn guard_error_redacts_secret_query() {
    let err = check_url(
        &Url::parse("https://evil.example/x?apikey=s3cret").unwrap(),
        WALLHAVEN,
    )
    .unwrap_err();
    assert!(!err.to_string().contains("s3cret"), "{err}");
}

#[test]
fn guard_rejects_non_443_port() {
    assert!(matches!(
        strict("https://w.wallhaven.cc:8443/"),
        Err(GuardError::Port { .. })
    ));
}

#[test]
fn guard_rejects_suffix_without_label_boundary() {
    assert!(matches!(
        strict("https://evilwallhaven.cc/"),
        Err(GuardError::UnlistedHost { .. })
    ));
}

#[test]
fn guard_rejects_unlisted_host() {
    assert!(matches!(
        strict("https://example.com/"),
        Err(GuardError::UnlistedHost { .. })
    ));
}

#[test]
fn guard_rejects_listed_host_as_prefix() {
    assert!(matches!(
        strict("https://wallhaven.cc.evil.example/"),
        Err(GuardError::UnlistedHost { .. })
    ));
}

#[test]
fn guard_rejects_everything_with_empty_allowlist() {
    let url = Url::parse("https://w.wallhaven.cc/").unwrap();
    assert!(matches!(
        check_url(&url, &[] as &[&str]),
        Err(GuardError::UnlistedHost { .. })
    ));
}

#[test]
fn guard_empty_suffix_matches_nothing() {
    let url = Url::parse("https://w.wallhaven.cc/").unwrap();
    assert!(check_url(&url, &[""]).is_err());
}

#[test]
fn guard_permissive_accepts_loopback_http() {
    let url = Url::parse("http://127.0.0.1:8080/x").unwrap();
    assert!(
        GuardConfig::permissive_loopback()
            .check_url(&url, &["127.0.0.1"])
            .is_ok()
    );
}

#[test]
fn guard_permissive_still_rejects_private_literal() {
    let url = Url::parse("http://10.0.0.1/x").unwrap();
    assert!(
        GuardConfig::permissive_loopback()
            .check_url(&url, &["10.0.0.1"])
            .is_err()
    );
}

#[test]
fn guard_permissive_still_enforces_allowlist() {
    let url = Url::parse("http://localhost:8080/x").unwrap();
    assert!(matches!(
        GuardConfig::permissive_loopback().check_url(&url, &["127.0.0.1"]),
        Err(GuardError::UnlistedHost { .. })
    ));
}

#[test]
fn guard_blocks_private_and_special_ranges() {
    let blocked = [
        "10.1.2.3",
        "172.16.0.1",
        "172.31.255.255",
        "192.168.1.1",
        "127.0.0.1",
        "127.255.0.1",
        "169.254.169.254",
        "100.64.0.1",
        "100.127.255.255",
        "0.0.0.0",
        "0.1.2.3",
        "224.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "fc00::1",
        "fd12:3456::1",
        "fe80::1",
        "ff02::1",
        "::ffff:127.0.0.1",
        "::ffff:8.8.8.8",
        "64:ff9b::10.0.0.1",
    ];
    let missed: Vec<_> = blocked.iter().filter(|s| !is_blocked_ip(ip(s))).collect();
    assert!(missed.is_empty(), "not blocked: {missed:?}");
}

#[test]
fn guard_allows_public_addresses() {
    let public = [
        "8.8.8.8",
        "1.1.1.1",
        "172.32.0.1",
        "100.128.0.1",
        "151.101.1.140",
        "2606:4700::1111",
        "64:ff9b::808:808",
    ];
    let blocked: Vec<_> = public.iter().filter(|s| is_blocked_ip(ip(s))).collect();
    assert!(blocked.is_empty(), "wrongly blocked: {blocked:?}");
}

/// Builds `/r1 → /r2 → … → /r{hops} → /ok` on `server`.
async fn redirect_chain(server: &MockServer, hops: usize) -> Url {
    for i in 1..=hops {
        let next = if i == hops {
            "/ok".to_owned()
        } else {
            format!("/r{}", i + 1)
        };
        Mock::given(method("GET"))
            .and(path(format!("/r{i}")))
            .respond_with(ResponseTemplate::new(302).insert_header("Location", next.as_str()))
            .mount(server)
            .await;
    }
    Mock::given(method("GET"))
        .and(path("/ok"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(server)
        .await;
    format!("{}/r1", server.uri()).parse().unwrap()
}

fn loopback_client() -> reqwest::Client {
    guarded_client(GuardConfig::permissive_loopback(), &["127.0.0.1"]).unwrap()
}

/// The [`GuardError`] somewhere in `err`'s source chain.
fn guard_cause(err: &reqwest::Error) -> Option<&GuardError> {
    GuardError::find_in(err)
}

#[tokio::test]
async fn guard_client_follows_three_redirects() {
    let server = MockServer::start().await;
    let url = redirect_chain(&server, 3).await;
    let response = loopback_client().get(url).send().await.unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn guard_client_rejects_fourth_redirect() {
    let server = MockServer::start().await;
    let url = redirect_chain(&server, 4).await;
    let err = loopback_client().get(url).send().await.unwrap_err();
    assert!(
        matches!(guard_cause(&err), Some(GuardError::TooManyRedirects { .. })),
        "{err:?}"
    );
}

#[tokio::test]
async fn guard_client_rejects_redirect_to_unlisted_host() {
    let server = MockServer::start().await;
    let port = server.address().port();
    Mock::given(method("GET"))
        .and(path("/r1"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("http://localhost:{port}/ok").as_str()),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/ok"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;
    let url: Url = format!("{}/r1", server.uri()).parse().unwrap();
    let err = loopback_client().get(url).send().await.unwrap_err();
    assert!(
        matches!(guard_cause(&err), Some(GuardError::UnlistedHost { .. })),
        "{err:?}"
    );
}

#[tokio::test]
async fn guard_strict_resolver_blocks_localhost_name() {
    // `localhost` resolves to loopback only; the strict resolver must drop every address.
    // The URL is never checked by `check_url` here, so this exercises the resolver alone.
    let client = guarded_client(GuardConfig::strict(), &["localhost"]).unwrap();
    let err = client.get("https://localhost:1/").send().await.unwrap_err();
    assert!(
        matches!(guard_cause(&err), Some(GuardError::BlockedAddress { .. })),
        "{err:?}"
    );
}

#[tokio::test]
async fn guard_permissive_resolver_keeps_loopback_name() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/ok"))
        .respond_with(ResponseTemplate::new(200))
        .mount(&server)
        .await;
    let port = server.address().port();
    let client = guarded_client(GuardConfig::permissive_loopback(), &["localhost"]).unwrap();
    let response = client
        .get(format!("http://localhost:{port}/ok"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

// ─── GuardedHttp: one client per fixed host list ──────────────────────────────────────────

fn row(hosts: &[&str]) -> GuardedHttp {
    GuardedHttp::new(GuardConfig::strict(), hosts).unwrap()
}

fn row_check(http: &GuardedHttp, url: &str) -> Result<(), GuardError> {
    http.check_url(&Url::parse(url).unwrap())
}

#[test]
fn guard_row_accepts_listed_host() {
    assert!(row_check(&row(&["example.com"]), "https://example.com/a.jpg").is_ok());
}

#[test]
fn guard_row_accepts_subdomain_of_listed_host() {
    assert!(row_check(&row(&["example.com"]), "https://img.example.com/a.jpg").is_ok());
}

#[test]
fn guard_row_rejects_sibling_suffix() {
    assert!(matches!(
        row_check(&row(&["example.com"]), "https://evilexample.com/a.jpg"),
        Err(GuardError::UnlistedHost { .. })
    ));
}

#[test]
fn guard_row_still_runs_the_strict_checks() {
    assert!(matches!(
        row_check(&row(&["example.com"]), "http://example.com/a.jpg"),
        Err(GuardError::Scheme { .. })
    ));
}

#[test]
fn guard_row_dedupes_hosts_ignoring_case() {
    let http = row(&["example.com", "EXAMPLE.com", "cdn.test", "example.com"]);
    assert_eq!(http.hosts(), ["example.com", "cdn.test"]);
}

#[test]
fn guard_row_keeps_at_most_max_row_hosts() {
    let hosts: Vec<String> = (0..MAX_ROW_HOSTS + 10)
        .map(|i| format!("h{i}.test"))
        .collect();
    let http = GuardedHttp::new(GuardConfig::strict(), &hosts).unwrap();
    assert_eq!(http.hosts(), &hosts[..MAX_ROW_HOSTS]);
}

#[test]
fn guard_row_refuses_a_host_past_the_cap() {
    let hosts: Vec<String> = (0..=MAX_ROW_HOSTS).map(|i| format!("h{i}.test")).collect();
    let http = GuardedHttp::new(GuardConfig::strict(), &hosts).unwrap();
    let last = format!("https://h{MAX_ROW_HOSTS}.test/a.jpg");
    assert!(matches!(
        row_check(&http, &last),
        Err(GuardError::UnlistedHost { .. })
    ));
}

#[test]
fn guard_row_reports_its_config() {
    let http = GuardedHttp::new(GuardConfig::permissive_loopback(), &["127.0.0.1"]).unwrap();
    assert_eq!(http.config(), GuardConfig::permissive_loopback());
}

#[test]
fn guard_row_debug_shows_the_hosts() {
    let text = format!("{:?}", row(&["example.com"]));
    assert!(text.contains("example.com"), "{text}");
}

fn loopback_row(hosts: &[&str]) -> GuardedHttp {
    GuardedHttp::new(GuardConfig::permissive_loopback(), hosts).unwrap()
}

#[tokio::test]
async fn guard_row_client_follows_max_redirects() {
    let server = MockServer::start().await;
    let url = redirect_chain(&server, MAX_REDIRECTS).await;
    let http = loopback_row(&["127.0.0.1"]);
    http.check_url(&url).unwrap();
    let response = http.client().get(url).send().await.unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn guard_row_client_rejects_more_than_max_redirects() {
    let server = MockServer::start().await;
    let url = redirect_chain(&server, MAX_REDIRECTS + 1).await;
    let err = loopback_row(&["127.0.0.1"])
        .client()
        .get(url)
        .send()
        .await
        .unwrap_err();
    assert!(
        matches!(guard_cause(&err), Some(GuardError::TooManyRedirects { .. })),
        "{err:?}"
    );
}

/// Mounts `/away` on `server`, redirecting to `target` + `/ok`, and an `/ok` that must not
/// be reached. Returns the `/away` URL on `from_host`.
async fn redirect_away(server: &MockServer, from_host: &str, target_host: &str) -> Url {
    let port = server.address().port();
    Mock::given(method("GET"))
        .and(path("/away"))
        .respond_with(ResponseTemplate::new(302).insert_header(
            "Location",
            format!("http://{target_host}:{port}/ok").as_str(),
        ))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/ok"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(server)
        .await;
    format!("http://{from_host}:{port}/away").parse().unwrap()
}

#[tokio::test]
async fn guard_row_client_rejects_redirect_to_unlisted_host() {
    let server = MockServer::start().await;
    let url = redirect_away(&server, "127.0.0.1", "localhost").await;
    let err = loopback_row(&["127.0.0.1"])
        .client()
        .get(url)
        .send()
        .await
        .unwrap_err();
    assert!(
        matches!(guard_cause(&err), Some(GuardError::UnlistedHost { .. })),
        "{err:?}"
    );
}

/// Two rows with different lists share nothing: each refuses the other's host, both before
/// the first request and on a redirect.
#[tokio::test]
async fn guard_rows_are_isolated() {
    let server = MockServer::start().await;
    let port = server.address().port();
    let ip_row = loopback_row(&["127.0.0.1"]);
    let name_row = loopback_row(&["localhost"]);

    let by_name: Url = format!("http://localhost:{port}/ok").parse().unwrap();
    let by_ip: Url = format!("http://127.0.0.1:{port}/ok").parse().unwrap();
    let first_url = (
        ip_row.check_url(&by_name).is_err(),
        name_row.check_url(&by_ip).is_err(),
    );

    let to_name = redirect_away(&server, "127.0.0.1", "localhost").await;
    let ip_err = ip_row.client().get(to_name).send().await.unwrap_err();
    let to_ip: Url = format!("http://localhost:{port}/away2").parse().unwrap();
    Mock::given(method("GET"))
        .and(path("/away2"))
        .respond_with(
            ResponseTemplate::new(302)
                .insert_header("Location", format!("http://127.0.0.1:{port}/ok").as_str()),
        )
        .mount(&server)
        .await;
    let name_err = name_row.client().get(to_ip).send().await.unwrap_err();
    let redirects = (
        matches!(guard_cause(&ip_err), Some(GuardError::UnlistedHost { .. })),
        matches!(
            guard_cause(&name_err),
            Some(GuardError::UnlistedHost { .. })
        ),
    );

    assert_eq!((first_url, redirects), ((true, true), (true, true)));
}

// ─── GuardedHttp::any_public_host: a user-typed link ─────────────────────────────────────

fn any_host(config: GuardConfig) -> GuardedHttp {
    GuardedHttp::any_public_host(config).unwrap()
}

fn any_host_check(config: GuardConfig, url: &str) -> Result<(), GuardError> {
    any_host(config).check_url(&Url::parse(url).unwrap())
}

#[test]
fn guard_any_host_accepts_any_public_https_host() {
    let accepted = ["https://example.com/a", "https://science.nasa.gov/apod/"]
        .map(|url| any_host_check(GuardConfig::strict(), url).is_ok());
    assert_eq!(accepted, [true, true]);
}

#[test]
fn guard_any_host_lists_no_hosts() {
    assert!(any_host(GuardConfig::strict()).hosts().is_empty());
}

#[test]
fn guard_any_host_rejects_http_in_strict_mode() {
    let err = any_host_check(GuardConfig::strict(), "http://example.com/a").unwrap_err();
    assert!(matches!(err, GuardError::Scheme { .. }), "{err:?}");
}

#[test]
fn guard_any_host_rejects_ip_literals_in_strict_mode() {
    let errs = [
        "https://10.0.0.1/a",
        "https://127.0.0.1/a",
        "https://[::1]/a",
    ]
    .map(|url| any_host_check(GuardConfig::strict(), url));
    assert!(
        errs.iter()
            .all(|e| matches!(e, Err(GuardError::IpLiteral { .. }))),
        "{errs:?}"
    );
}

#[test]
fn guard_any_host_rejects_localhost_credentials_and_ports_in_strict_mode() {
    let errs = [
        "https://localhost/a",
        "https://user:pw@example.com/a",
        "https://example.com:8443/a",
    ]
    .map(|url| any_host_check(GuardConfig::strict(), url));
    assert!(
        matches!(
            errs,
            [
                Err(GuardError::Localhost { .. }),
                Err(GuardError::Credentials { .. }),
                Err(GuardError::Port { .. }),
            ]
        ),
        "{errs:?}"
    );
}

#[tokio::test]
async fn guard_any_host_client_follows_a_redirect_to_another_host() {
    let server = MockServer::start().await;
    let port = server.address().port();
    Mock::given(method("GET"))
        .and(path("/moved"))
        .respond_with(
            ResponseTemplate::new(301)
                .insert_header("Location", format!("http://localhost:{port}/here").as_str()),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/here"))
        .respond_with(ResponseTemplate::new(200))
        .expect(1)
        .mount(&server)
        .await;
    let http = any_host(GuardConfig::permissive_loopback());
    let url: Url = format!("http://127.0.0.1:{port}/moved").parse().unwrap();
    http.check_url(&url).unwrap();

    let response = http.client().get(url).send().await.unwrap();

    assert_eq!(
        (response.status().as_u16(), response.url().host_str()),
        (200, Some("localhost"))
    );
}

/// The redirect policy runs the address checks: a hop to a private IP literal is refused even
/// in permissive mode, which allows only loopback literals.
#[tokio::test]
async fn guard_any_host_client_rejects_a_private_ip_hop() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/inside"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "http://10.0.0.1/admin"))
        .mount(&server)
        .await;
    let url: Url = format!("{}/inside", server.uri()).parse().unwrap();

    let err = any_host(GuardConfig::permissive_loopback())
        .client()
        .get(url)
        .send()
        .await
        .unwrap_err();

    assert!(
        matches!(guard_cause(&err), Some(GuardError::IpLiteral { .. })),
        "{err:?}"
    );
}

#[tokio::test]
async fn guard_any_host_client_follows_max_redirects() {
    let server = MockServer::start().await;
    let url = redirect_chain(&server, MAX_REDIRECTS).await;
    let response = any_host(GuardConfig::permissive_loopback())
        .client()
        .get(url)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}

#[tokio::test]
async fn guard_any_host_client_rejects_more_than_max_redirects() {
    let server = MockServer::start().await;
    let url = redirect_chain(&server, MAX_REDIRECTS + 1).await;
    let err = any_host(GuardConfig::permissive_loopback())
        .client()
        .get(url)
        .send()
        .await
        .unwrap_err();
    assert!(
        matches!(guard_cause(&err), Some(GuardError::TooManyRedirects { .. })),
        "{err:?}"
    );
}
