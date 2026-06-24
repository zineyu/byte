set shell := ["bash", "-euo", "pipefail", "-c"]

# Run the full local validation suite mirroring .github/workflows/ci.yml.
verify: repo-hygiene workflow-syntax rust desktop audit

# Check required docs, Markdown sanity, and obvious committed secrets.
repo-hygiene:
    #!/usr/bin/env bash
    set -euo pipefail

    required_files=(
      AGENTS.md
      CONTEXT.md
      docs/architecture/mvp-architecture.md
      docs/agents/domain.md
      docs/agents/issue-tracker.md
      docs/agents/triage-labels.md
    )

    for file in "${required_files[@]}"; do
      if [ ! -s "$file" ]; then
        echo "Required documentation file is missing or empty: $file" >&2
        exit 1
      fi
    done

    adr_count="$(find docs/adr -maxdepth 1 -type f -name '*.md' | wc -l | tr -d ' ')"
    if [ "$adr_count" -lt 7 ]; then
      echo "Expected at least 7 ADR files, found $adr_count" >&2
      exit 1
    fi

    found=0
    while IFS= read -r -d '' file; do
      found=1
      if [ ! -s "$file" ]; then
        echo "Markdown file is empty: $file" >&2
        exit 1
      fi
      if grep -n $'\r' "$file" >/tmp/byte-crlf-lines.txt; then
        echo "Markdown file contains CRLF line endings: $file" >&2
        cat /tmp/byte-crlf-lines.txt >&2
        exit 1
      fi
    done < <(find . \
      -path './.git' -prune -o \
      -path './.jj' -prune -o \
      -path './target' -prune -o \
      -path './apps/desktop/node_modules' -prune -o \
      -path './apps/desktop/dist' -prune -o \
      -path './apps/desktop/src-tauri/target' -prune -o \
      -type f -name '*.md' -print0)

    if [ "$found" -eq 0 ]; then
      echo "No Markdown files found" >&2
      exit 1
    fi

    if grep -RInE \
      --exclude-dir=.git \
      --exclude-dir=.jj \
      --exclude-dir=target \
      --exclude-dir=node_modules \
      --exclude-dir=dist \
      --exclude='*.lock' \
      '(OPENAI_API_KEY|ANTHROPIC_API_KEY|AWS_SECRET_ACCESS_KEY|SECRET_KEY|PRIVATE_KEY|GITHUB_TOKEN)=[^[:space:]]+' \
      .; then
      echo "Potential committed secret detected" >&2
      exit 1
    fi

# Parse GitHub Actions workflow YAML files.
workflow-syntax:
    ruby -e 'require "yaml"; Dir[".github/workflows/*.{yml,yaml}"].sort.each { |file| YAML.load_file(file); puts "parsed #{file}" }'

# Run all Rust quality gates.
rust: rust-fmt rust-clippy rust-test

rust-fmt:
    cargo fmt --all -- --check

rust-clippy:
    cargo clippy --workspace --all-targets -- -D warnings

rust-test:
    cargo test --workspace --all-targets

# Install desktop dependencies exactly as CI does when pnpm is unavailable;
# prefer an existing local pnpm binary to support immutable Nix environments.
desktop-install:
    #!/usr/bin/env bash
    set -euo pipefail
    cd apps/desktop
    if ! command -v pnpm >/dev/null 2>&1; then
      corepack enable
      corepack prepare pnpm@latest --activate
    fi
    pnpm install --frozen-lockfile

# Run desktop web gates. Missing scripts are skipped to match CI behavior.
desktop: desktop-install
    #!/usr/bin/env bash
    set -euo pipefail
    cd apps/desktop

    run_script() {
      local script="$1"
      if node -e "const scripts = require('./package.json').scripts || {}; process.exit(scripts[process.argv[1]] ? 0 : 1)" "$script"; then
        pnpm run "$script"
      else
        echo "No package.json script named '$script'; skipping."
      fi
    }

    run_script lint
    run_script typecheck
    run_script test
    run_script build

# Run dependency audit for the desktop package.
audit: desktop-install
    cd apps/desktop && pnpm audit --audit-level high

# Start the desktop app in development mode.
dev:
    cd apps/desktop && pnpm run tauri:dev

# Build the local daemon used by the desktop app.
build-daemon:
    cargo build -p byte-daemon
