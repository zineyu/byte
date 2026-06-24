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
just repo-hygiene
just workflow-syntax

# Rust formatting, linting, and tests
just rust
just rust-fmt
just rust-clippy
just rust-test

# Desktop frontend install, typecheck/test/build, and audit
just desktop
just audit
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
