# Provider fixtures (captured from the Go providers)

`go run ./tools/goldens` writes everything here except this README
(`tools/goldens/providers.go`). Do not edit the files by hand; change the
generator and re-run it.

## Layout

`<provider>/` holds two kinds of file:

- Body files (`*.json`, `*.html`, `*.txt`): the exact bytes an HTTP fixture
  server sends. For bing, reddit and wallhaven they are the bodies from the
  Go `httptest` handlers in `internal/providers/*/*_test.go`, with the same
  values and the same encoder (`json.NewEncoder(w).Encode`, so there is a
  trailing `\n`). Go has no tests for apod and unsplash, so their bodies are
  built from the structs in `apod.go` and `unsplash.go`.
- `<case>.case.json`: one scenario, run through the real Go provider. Its
  fields are:

| Field | Meaning |
|---|---|
| `go_test` | The Go test this case mirrors. For apod and unsplash it reads `synthesised (<file> has no tests)`. |
| `provider` | Provider name. |
| `config` | The provider's config section, as TOML keys (`api_key`, `subreddits`, ...). |
| `query` | The argument to `Fetch`, or `""`. |
| `routes` | What the fixture server answers. A request matches a route on `method` + `host` + `path`; the query string is ignored. The server sends `status`, `content_type`, any `headers`, then `body_file` from the same dir. An unmatched request gets 404, and the generator fails if that happens. |
| `requests` | Every request Go made: method, full URL (scheme + Host + request URI), User-Agent. Sorted by method+URL, because some providers fetch concurrently. |
| `sort_results` | `true` when Go shuffles the results (wallhaven scrape). The recorded results are sorted by `source_url`; sort the Rust results the same way before comparing. |
| `expected` | Go's output: either `{"results": [...]}` (possibly `[]`) with `source_url`, `image_url`, `title`, `author`, `source` per result, or `{"error": "<Go error text>"}`. |

## Capture method

The generator replaces the dialer in `http.DefaultTransport`, so every
connection goes to a local `httptest` server: port 443 goes to a TLS server
and all other ports go to a plain one. The Go code keeps its real hostnames
(`www.bing.com`, `wallhaven.cc`, ...), so the `Host` header and the recorded
URLs are the production ones. No request leaves the machine.

## Notes for the Rust tests

- `{{HOST}}` in a body file stands for the request's `Host` header and is
  replaced when the body is served. `wallhaven/scrape_search.html` uses it
  for its thumbnail links (`http://{{HOST}}/w/abc123`). The Go run served
  them as `http://wallhaven.cc/...`, which is why the scrape results'
  `source_url` starts with `http://`. A Rust mock server on `127.0.0.1:PORT`
  should either serve the host `wallhaven.cc` or replace its own origin
  with `http://wallhaven.cc` before comparing.
- `user_agent` is `Go-http-client/1.1` for bing, because the bing provider
  sets no UA and Go's default applies. That is not a contract, so do not
  assert it. The other providers send `wallpaperd/1.0` explicitly; assert
  that.
- Error strings are Go `error.Error()` text. Status-based errors
  (`bing: unexpected status 500`, `reddit: unexpected status 403`, the
  apod rate limit, the unsplash 401) are product messages, so match them
  exactly. Errors that wrap a transport or parse error would contain Go
  wording; none of the current cases do.
- Expected results contain no times, random values or temp paths, so the
  fixtures are byte-stable across runs.
