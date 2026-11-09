# Makefile for wallpaperd Settings GUI (Wails + React + TypeScript)
#
# Prerequisites:
#   - Go 1.21+
#   - Node.js 16+
#   - Wails CLI v2: go install github.com/wailsapp/wails/v2/cmd/wails@latest
#
# Usage:
#   make dev             — Start hot-reload dev server
#   make build           — Produce build/bin/wallpaperd-settings.app
#   make build-universal — macOS universal binary (arm64 + amd64)
#   make clean           — Remove build artefacts and node_modules

.PHONY: dev build build-universal build-package install-deps lint clean check-wails

# ─── Tools ─────────────────────────────────────────────────────────────────────

WAILS := $(shell command -v wails 2>/dev/null)
NPM   := $(shell command -v npm 2>/dev/null)

check-wails:
ifndef WAILS
	$(error "wails not found — install with: go install github.com/wailsapp/wails/v2/cmd/wails@latest")
endif
ifndef NPM
	$(error "npm not found — install Node.js from https://nodejs.org")
endif

# ─── Development ───────────────────────────────────────────────────────────────

## dev: Launch the Wails dev server with hot-module replacement.
## The React frontend reloads on file changes; Go backend recompiles on save.
dev: check-wails
	wails dev

# ─── Production builds ─────────────────────────────────────────────────────────

## build: Compile a release build for the host architecture.
## Output: build/bin/wallpaperd-settings.app  (macOS .app bundle)
build: check-wails
	wails build

## build-universal: Compile a universal macOS binary (arm64 + amd64).
## The output .app runs natively on both Apple Silicon and Intel Macs.
build-universal: check-wails
	wails build -platform darwin/universal

## build-package: Build and wrap the .app in a distributable .zip.
build-package: check-wails
	wails build -platform darwin/universal -package

# ─── Frontend helpers ──────────────────────────────────────────────────────────

## install-deps: Install Node dependencies without running a full build.
install-deps:
	cd frontend && npm install

## lint: Run TypeScript type-check and ESLint over the frontend source.
lint: install-deps
	cd frontend && npm run typecheck 2>/dev/null || npx tsc --noEmit
	cd frontend && npm run lint 2>/dev/null || true

# ─── Housekeeping ──────────────────────────────────────────────────────────────

## clean: Remove build outputs and cached frontend artefacts.
clean:
	rm -rf build/bin
	rm -rf frontend/dist
	rm -rf frontend/node_modules

## help: Print this help message.
help:
	@grep -E '^## ' $(MAKEFILE_LIST) | sed 's/^## //' | column -t -s ':'
