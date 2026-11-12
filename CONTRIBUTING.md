# Contributing to kakemono

The user documentation is at [kakemono.rielj.xyz](https://kakemono.rielj.xyz), from the [`RielJ/kakemono-docs`](https://github.com/RielJ/kakemono-docs) repository. This file covers building, testing and releasing kakemono itself.

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cd crates/ui/frontend && npm ci && npm run fmt:check && npm run lint && npm run typecheck && npm test && npm run build
```

The frontend uses [oxlint](https://oxc.rs/docs/guide/usage/linter) (`.oxlintrc.json`) and [oxfmt](https://oxc.rs/docs/guide/usage/formatter) (`.oxfmtrc.json`); `npm run fmt` formats it. Both skip the generated `src/shared/bindings`.

`scripts/gate.sh full` runs the Rust checks that CI runs, plus cross-target clippy. To run CI's Linux and Windows jobs from a Mac before you push:

| Script | What it runs | Needs |
|---|---|---|
| `scripts/linux-check.sh` | The Rust tests, the frontend checks, and a `.deb` build, in an Ubuntu 24.04 container | Docker |
| `scripts/windows-check.sh` | Clippy and the Rust tests in a Windows 11 ARM64 VM | QEMU (`brew install qemu`) and about 60 GB of disk |

To create the Windows VM, download the Windows 11 ARM64 ISO from [Microsoft](https://www.microsoft.com/software-download/windows11arm64), then run `scripts/windows-vm/vm.sh install <iso>` and `scripts/windows-vm/vm.sh setup`. Both run unattended and take about an hour in total. The VM is in `~/VMs/kakemono-win`. To remove it, run `vm.sh destroy`. Run either script with `--help` to see its options.

## Releasing

1. Bump `version` in `Cargo.toml` and run `npm version <version> --no-git-tag-version` in `crates/ui/frontend`. Commit, then push a `v<version>` tag.
2. The `release` workflow builds every platform and creates a draft release with the files and `SHA256SUMS`. Check the draft, then publish it.
3. Publishing the release starts the `packages` workflow. It fills in the templates in `packaging/` from the release's `SHA256SUMS` with `scripts/package-manifests.sh`, then pushes the Homebrew tap, the Scoop bucket and the AUR package, and opens a winget pull request. Pre-releases are skipped. To run it again for a tag, start it by hand from the Actions tab.

To render the manifests locally, for example to check a template change:

```sh
gh release download v<version> -p SHA256SUMS
scripts/package-manifests.sh <version> SHA256SUMS out
```

Each channel needs a one-time setup. A channel whose secret is missing is skipped with a notice, so you can set them up one at a time.

| Channel | One-time setup | Repository secret |
|---|---|---|
| In-app updates | Generate the updater key with `npx tauri signer generate`. Its public key goes in `plugins.updater.pubkey` in `crates/ui/src-tauri/tauri.conf.json`. | `TAURI_SIGNING_PRIVATE_KEY`, `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` |
| Homebrew, Scoop | Create the public repositories `RielJ/homebrew-tap` and `RielJ/scoop-bucket`. Create a fine-grained token with **Contents: read and write** on both. | `PACKAGES_TOKEN` |
| AUR | Create an account on [aur.archlinux.org](https://aur.archlinux.org) and add an SSH public key to it. | `AUR_SSH_KEY` (the private key) |
| winget | Fork [`microsoft/winget-pkgs`](https://github.com/microsoft/winget-pkgs). Create a classic token with the `public_repo` scope. | `WINGET_TOKEN` |

winget needs one version in `microsoft/winget-pkgs` before the workflow can submit updates. Submit the first version by hand: render the manifests as shown above, copy `out/winget/` to `manifests/r/RielJ/kakemono/<version>/` in your fork, and open a pull request.

Scoop installs `kakemono-windows-x64-portable.zip`, which releases have shipped since 0.3.1. The [Scoop install page](https://kakemono.rielj.xyz/install/scoop/) gives two commands. Adding the bucket needs Git; installing from the bucket's raw manifest URL doesn't. For a URL install, Scoop records the URL, so `scoop update kakemono` and `scoop status` read the newest manifest from the bucket.

## Code layout

| Crate | Contents |
|---|---|
| `crates/cli` | The `kakemono` binary: commands and the daemon host |
| `crates/engine` | Rotation, history, favorites, trash, and hooks |
| `crates/sources` | The wallpaper sources and downloads |
| `crates/platform` | Wallpaper setters, theme and display signals, login services, and Linux backend detection |
| `crates/config` | The config schema, defaults, paths, and editing |
| `crates/protocol` | The socket protocol and client |
| `crates/ui` | The Tauri app: tray, picker, and settings |

