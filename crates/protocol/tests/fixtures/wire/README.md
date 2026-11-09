# IPC wire goldens (captured from the Go daemon)

`tools/goldens/replay.sh` wrote these files. It built the Go daemon from the
`go-final` tree, ran it on a scratch socket with a scratch `HOME` and config,
and recorded the raw bytes of each exchange. Do not edit them by hand.

The Go code and `tools/goldens/` were deleted at the cutover (Task 49), so the
files are now frozen. To regenerate them, check out `tools/goldens/` from a
commit before the cutover, for example `git checkout 8741397 -- tools/goldens`.

## Files

- `NN_<case>.request.jsonl`: the exact bytes the client wrote. Most cases are
  one line. `22_multi_command_session` sends six lines on one connection,
  including a blank line and a line with extra whitespace.
- `NN_<case>.response.jsonl`: the exact bytes the Go daemon wrote back. There
  is one response line per request line, in order, and each ends in `\n`
  (`json.Encoder.Encode`).
- `state_after_replay.json`: the `state.json` the daemon wrote on SIGTERM
  after all cases ran (`State.Save` bytes: two-space `MarshalIndent`, no
  trailing newline).

`NN` is the order in which the cases were sent. Cases depend on earlier ones
(the history grows), so replay them in order against one daemon.

## Daemon setup during capture

- Only the `local` source is enabled. Its only directory is
  `/tmp/wpd-goldens-replay/pics`, which holds one image (`alpha.png`), so every
  provider fetch returns that image.
- `set` uses `/tmp/wpd-goldens-replay/set/beta.png`.
- Theme detection always reports `light`, because a shim `defaults` exits 1.
- `interval` is `"30m"` and rotation is disabled.

## Nondeterministic fields (normalise before comparing)

| Field | Where | Why it varies | How to normalise |
|---|---|---|---|
| `set_at` | every `WallpaperInfo` (`data.set_at`, `data.current.set_at`, `state_after_replay.json` history entries) | Go uses `time.Now().Format(time.RFC3339)` in the **local** time zone. Values differ on every run, and the offset (`+08:00`, `Z`, …) depends on the machine's TZ. | Replace with a placeholder (for example `"<SET_AT>"`) after checking it parses as RFC 3339 with whole seconds. |
| `last_changed` | `state_after_replay.json` only | `time.Now()` with nanoseconds and local offset (`time.Time` JSON, RFC 3339 with up to 9 fractional digits). | Replace with a placeholder after checking it parses as RFC 3339. |

All other bytes are stable between runs.

Paths are stable because the scratch dir always has the same name, but they
are absolute: `/tmp/wpd-goldens-replay/...`. A Rust replay test that runs its
own daemon in a different temp dir should replace its own root with
`/tmp/wpd-goldens-replay` (or both with `<ROOT>`) before it compares.
This affects `path`, `source_url` (`file://<ROOT>/...`), `image_url`, and the
request line of `06_set`.

## Behaviours worth asserting in Rust

- An empty line gets `{"ok":false,"error":"invalid JSON: unexpected end of JSON input"}`,
  and the connection stays open (`22_multi_command_session`).
- Unknown fields are ignored, and `"args":null` is accepted (`21_extra_fields`).
- A missing `cmd` is dispatched as `""`, which gives `unknown command: ""`
  (`20_missing_cmd_field`).
- A `set` whose `path` is not a string reports the same error as a missing
  path (`12_set_path_not_string`).
- `pause`, `resume` and `favorite` answer `{"ok":true}`, with no `data` key
  (`omitempty`). `status`, `next`, `prev`, `set` and `trash` always include
  `data`.
- `WallpaperInfo` from `set` has `source_url` and `image_url` as `""`, not
  absent. No `WallpaperInfo` field uses `omitempty`.
- `current` is `null` (key present) before the first wallpaper.
- Go's encoder escapes `&`, `<` and `>` as `\u0026`, `\u003c` and `\u003e`.
  Compare semantically, or make the Rust encoder match on purpose.
- Error strings are Go `error.Error()` text. The `invalid JSON: ...` detail is
  Go's `encoding/json` wording. Rust's serde wording will differ, so assert
  only the `invalid JSON: ` prefix. In the same way, `13_set_nonexistent`
  ends in Go's `os.Stat` text (`stat <path>: no such file or directory`);
  assert the `manager: file not found: ` prefix only.
