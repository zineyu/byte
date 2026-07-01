# Development Commands

Common commands for building, testing, and running Byte Agent.

## Full local verification

The root `Justfile` mirrors the quality gates in `.github/workflows/ci.yml`.

```bash
# Run repository hygiene, workflow syntax, Rust gates, desktop gates, and audit
just verify
```

## Individual quality gates

```bash
# Repository and workflow checks
just verify repo
just verify workflow

# Rust formatting, linting, and tests
# Runs cargo fmt --check, clippy, and tests.
just verify rust

# Desktop frontend format check, typecheck, tests, and build
# Runs Prettier check for TS/CSS/HTML before package scripts.
just verify desktop
just verify audit

# Format / format-check Rust and desktop frontend code
just fmt
just fmt-check
```

## Rust workspace

```bash
# Build the workspace
cargo build

# Run daemon integration tests directly
cargo test -p byte-daemon --test unix_socket_json_rpc
```

## Desktop application

```bash
cd apps/desktop

# Install dependencies
pnpm install

# Run in development mode from the repository root
just dev
```

Note: on the current development machine, Tauri needs the dynamic loader path:

```bash
LD_LIBRARY_PATH=/usr/lib pnpm run tauri:dev
```

## Linux system dependencies

Tauri's Rust side links against GTK, WebKitGTK, and related system libraries. On Debian/Ubuntu, install the development packages before building:

```bash
sudo apt-get update
sudo apt-get install -y \
  libwebkit2gtk-4.1-dev \
  libjavascriptcoregtk-4.1-dev \
  libsoup-3.0-dev \
  build-essential \
  curl \
  wget \
  libssl-dev \
  libglib2.0-dev \
  libgtk-3-dev \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  patchelf \
  pkg-config
```

These packages are also installed by the `rust` and `package-desktop` GitHub Actions jobs.
