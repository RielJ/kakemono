# Ubuntu 24.04 with what CI's ubuntu-24.04 jobs install: WebKitGTK for Tauri, the Rust
# toolchain from rust-toolchain.toml, and Node from .node-version. Used by scripts/linux-check.sh.
FROM ubuntu:24.04

ARG NODE_MAJOR
ARG DEBIAN_FRONTEND=noninteractive

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      build-essential ca-certificates curl file git pkg-config xz-utils \
      libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf \
 && rm -rf /var/lib/apt/lists/*

RUN arch="$(dpkg --print-architecture | sed 's/amd64/x64/')" \
 && base="https://nodejs.org/dist/latest-v${NODE_MAJOR}.x" \
 && file="$(curl -fsSL "$base/SHASUMS256.txt" | grep -o "node-v[0-9.]*-linux-$arch.tar.xz" | head -1)" \
 && curl -fsSL "$base/$file" -o "/tmp/$file" \
 && curl -fsSL "$base/SHASUMS256.txt" | grep " $file\$" | (cd /tmp && sha256sum -c -) \
 && tar -xJf "/tmp/$file" -C /usr/local --strip-components=1 \
 && rm "/tmp/$file"

# GitHub runs jobs as a non-root user; as root, the file-permission tests would pass vacuously.
# The cache mount points exist here so new named volumes take this user's ownership.
RUN useradd --create-home --uid 1001 runner \
 && mkdir -p /src/crates/ui/frontend/node_modules /target /home/runner/.cargo/registry \
 && chown -R runner:runner /src /target /home/runner
USER runner
ENV CARGO_HOME=/home/runner/.cargo \
    CARGO_TARGET_DIR=/target \
    PATH=/home/runner/.cargo/bin:$PATH

RUN curl -fsSL https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain none

WORKDIR /src
# Installs the toolchain rust-toolchain.toml names, so the image matches the source it checks.
COPY --chown=runner:runner rust-toolchain.toml /src/
RUN rustup show active-toolchain || rustup toolchain install
