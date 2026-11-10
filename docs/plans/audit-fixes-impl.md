# Audit fixes: implementation plan

This plan fixes every finding in `docs/plans/audit-2026-11.md` (finding ids such as H1, M4 and S2 refer to that
file). All of it lands before Task 27 of `variety-sources-impl.md`. The five full lane reports are in
`/tmp/kk-audit/*.md`, and every task below should read the matching report for the exact lines.

## Rules for every task

- **Idiomatic Rust.** Read `.pi/skills/rust-best-practices/SKILL.md` and the chapters in its `references/` that
  apply to the change. Tauri work also reads `.pi/skills/tauri-v2/SKILL.md` and its references. This is the
  user's standing preference.
- **Tests first.** Each bug fix starts with a test that fails for the reason in the finding. The report notes
  that it failed, then the fix makes it pass. Refactors keep the existing tests green and add none unless they
  cover a gap the audit named.
- **Behaviour and wire format.** Keep the Go-compatible output text and the IPC JSON unless the finding says
  otherwise. Golden and snapshot tests must not change unless the task says so.
- **Build isolation.** Each lane uses its own `CARGO_TARGET_DIR` under `~/.tmp/` and deletes it at the end.
  Lanes never share `target/`.
- **Other platforms.** Linux-only and Windows-only code is checked with
  `cargo clippy --target x86_64-unknown-linux-gnu` and `--target x86_64-pc-windows-msvc` (check only, no
  linking). Both targets are installed.
- **Gate.** A task is done when all of these pass:
  - `cargo test --workspace --all-features`
  - `cargo clippy --workspace --all-targets --all-features -- -D warnings`, for the host and for the other
    targets the task touched
  - `cargo fmt --all -- --check`
  - the frontend `tsc` and `vitest`, if the task touched the frontend or the bindings

## Phase A0: shared file helpers (runs alone first)

**Task F1: new crate `kakemono-fs`** (`crates/fs`), depending only on std (and `tempfile` as a dev-dependency).

- **API:**
  - `atomic_write(path, bytes)`:
    - writes through a unique `create_new` temp file in the target's folder, then fsyncs it;
    - keeps the old file's permissions (new files get mode 0600);
    - renames over the target, then fsyncs the folder;
    - if the target is a symlink, writes to the resolved target and leaves the link in place.
  - `PathLock`: an in-process lock keyed by the canonical path, held across a read-modify-write.
  - `update(path, |old: Option<&[u8]>| -> Result<Vec<u8>, E>)`: runs a whole read-modify-write under that lock
    and writes the result with `atomic_write`.
  - `copy_unique(src, dir)`: moved from `engine/src/files.rs:477`. It never overwrites; it adds a `-N` suffix.
- **Adopt it in `config/src/edit.rs`** (fixes H1 and M4). `apply_patch` and the other writers in that file run
  through `update`. `MIGRATE_LOCK` in `load.rs` uses the same `PathLock`.
- **Shared image size limit.** Add `pub const MAX_IMAGE_BYTES: u64 = 64 * 1024 * 1024` to
  `kakemono-sources::download`. It is only a constant: Task F3 enforces it, and Task F2 switches
  `engine/src/manual.rs` to use it.
- **Tests:**
  - two threads patching different keys of one file in a loop: the file always parses and both keys survive;
  - a symlinked config stays a symlink after a patch;
  - `copy_unique` never overwrites on a name collision;
  - unique temp names under concurrent writes;
  - permissions are kept.
- **Files owned:** `crates/fs/**`, the workspace `Cargo.toml` (members and the workspace dependency only),
  `crates/config/src/edit.rs`, `crates/config/src/load.rs` (the lock only), `crates/config/Cargo.toml`, and the
  constant in `crates/sources/src/download.rs`.

## Phase A: per-crate fixes (five lanes in parallel, after F1)

**Task F2: engine** (owns `crates/engine/**`):
- H7: a per-source async mutex around download and `record_seen`. The ledger is written with
  `kakemono_fs::atomic_write`.
- M2: the trim protects the pending pick.
- M3: the filter budget is spread round-robin across sources.
- M12 (engine part): `state.rs` persist and `pool.rs` `album_next` run off the runtime threads.
- `state.rs`, `facts.rs` and `files.rs` use `kakemono-fs`. `copy_unique` is re-exported from the new crate.
- `manual.rs` uses `MAX_IMAGE_BYTES`.
- Typed errors instead of `Fetch(Vec<String>)`, `Files(String)`, `Manual(String)`, and instead of the `String`
  results of `check_source` and `download_now`. `Display` text stays the same, because the daemon's wire errors
  depend on it.
- Split `engine.rs` into ops, error, deps and Go-compat text.
- Remove duplication: interval parsing (`rotation.rs:289` and `downloader.rs:308`), and `go_stat_error` with
  `go_reason`.
- Filters are compiled once and passed to the downloader.
- `resolve_local_path` becomes `pub(crate)`.
- Real-timer tests move to paused time; fixture helpers are shared.
- Dependencies and docs:
  - remove the unused `futures` and `insta`;
  - move `async-trait` to dev-dependencies;
  - fix the stale docs in `events.rs`, `lib.rs` and `pool::pick`;
  - the `unreachable!` in `manual.rs` becomes a typed error.
- Every other Medium and Low item in `/tmp/kk-audit/engine.md`.

**Task F3: sources** (owns `crates/sources/**`, apart from the F1 constant):
- H5: `connect_timeout` and `read_timeout` instead of a total deadline for image bodies. Downloads enforce
  `MAX_IMAGE_BYTES` while streaming.
- M1: a shared `check_status` helper maps 429 to `RateLimited` for every provider.
- M10: a shared `null_default` helper.
- The URL is redacted inside `DownloadError` itself.
- Base-URL trimming is done one way everywhere, and the request-build error arms that cannot fail are removed.
- Tests that reuse a port right after freeing it are fixed.
- Dependencies: remove `chrono`, `futures`, `insta` and the unused `live` feature.
- Every Low item in `/tmp/kk-audit/sources.md`.

**Task F4: platform** (owns `crates/platform/**`):
- H3: one `tokio::time::timeout` around every `set` in `ResilientSetter`, covering KDE, XFCE and Windows.
- H4: stop the new pid on both error paths, and write the record before stopping the old setter.
- M9: `process_group(0)` and `killpg` on timeout.
- Split `linux/mod.rs` into resilient and live.
- Remove duplication: `host()` (macOS and Windows) and the theme/signal hub (three versions).
- `SetError::Os` and `SetError::Dbus` keep the original error as their source.
- Windows rejects relative paths.
- Fix the reaper-thread spawn leak.
- Test doubles stay behind the `fake` feature.
- Every Low item in `/tmp/kk-audit/platform.md`.
- Clippy runs for both other targets.

**Task F5: config, protocol and cli** (owns `crates/config/**` apart from the F1 files, `crates/protocol/**` and
`crates/cli/**`):
- M5: `Unsupported(String)` keeps the raw type, and a missing `type` is an error.
- M6: a client that timed out is poisoned and must reconnect.
- M7: the daemon takes the single-instance lock before it migrates or creates the config.
- M8: `normalize_sources` warnings reach the load report and doctor.
- M11: no `{source}` in `#[error]` text across config, protocol and cli. The CLI test checks that each cause
  appears exactly once.
- CLI duplication: 7 one-call command modules, 3 runtime builders, 2 copies each of `absolute` and `detach`.
- The socket and runtime directory are computed in one place, in protocol or config.
- Remove `chrono` from protocol, and the stale "Task 40" comments in the workspace `Cargo.toml`.
- Every Low item in `/tmp/kk-audit/core.md`: socket mode on creation, a cap on connections, blocking calls on
  the runtime, `$EDITOR` splitting, and folder fsync.

**Task F6: Tauri** (owns `crates/ui/**`):
- H2: `copy_to_favorites` uses `kakemono_fs::copy_unique`, with a name-collision test.
- H6: `Fetch`, `DownloadNow` and `CheckSource` use their own connection, and a timeout gets its own error kind.
- S1:
  - remove `autostart:default`, `core:window:allow-close`, `opener:allow-reveal-item-in-dir` and the npm
    autostart plugin;
  - replace `opener:allow-open-path` with a Rust `open_folder` command that only opens folders;
  - give each window its own capability, with a command list per window through `AppManifest::commands`.
- S2: `favorite`, `set_file` and `reveal` require `Thumbs::is_admitted`.
- M12 (Tauri part): `app_info`, `show_picker`, `reveal` and the tray check in `setup` move off the main thread.
- One `{kind, message}` serializer instead of three, keeping the error kinds.
- Split `commands_sources.rs` and `commands.rs` as the report proposes.
- Export the Tauri IPC reply types with ts-rs instead of hand-written TS.
- Remove dead code (`cli_path`, and frontend commands that nothing calls).
- `thumb://` caching: no stale image after a file changes, and the cache is pruned.
- CSP: check whether `'unsafe-inline'` styles are needed, and set `devCsp`.
- Every Low item in `/tmp/kk-audit/tauri.md`.

## Phase B: shared protocol types (after F5 and F6)

**Task F7:** move the doctor payload and `Theme` into `kakemono-protocol`. `cli/src/daemon/server.rs` and
`ui/src-tauri/src/commands_desktop.rs` both use them, and the TS bindings are regenerated. The JSON wire format
stays the same.

## Phase C: lints and docs (after B)

**Task F8: lint config.**
- `[workspace.lints]`:
  - `rust`: `unsafe_code = "deny"`, with a reasoned `#[expect]` or `#[allow]` on each FFI module;
  - `clippy`: `unwrap_used`, `expect_used`, `panic`, `missing_errors_doc`, `missing_panics_doc`, `dbg_macro`,
    `todo` and `unimplemented`.
- `clippy.toml` with `allow-unwrap-in-tests`, `allow-expect-in-tests` and `allow-panic-in-tests`.
- CI runs clippy on all three operating systems; check `ci.yml` first.

**Tasks F9a–F9e:** fix every warning the new lints raise. There is one lane per crate group (engine, sources,
platform, config/protocol/cli, ui). The 236 missing `# Errors` sections get real documentation.

## Checklist

- [ ] F1 shared file helpers
- [ ] F2 engine
- [ ] F3 sources
- [ ] F4 platform
- [ ] F5 config, protocol and cli
- [ ] F6 Tauri
- [ ] F7 shared protocol types
- [ ] F8 lint config
- [ ] F9a–F9e lint fixes
