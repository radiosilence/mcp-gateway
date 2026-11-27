# mcp-gateway — an OAuth-fronted gateway for self-hosted MCP servers

# What CI checks: format, lint with warnings as errors, tests, the stylesheet, the workflows.
check:
    cargo fmt --all -- --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test --locked --all-features
    just css-fresh
    actionlint

# Compile the dashboard's stylesheet from styles/ and the kit's source. Committed because the binary embeds it; the
# binary serves the kit's typeface from the crate.
css: kit
    tailwindcss --optimize --input styles/app.css --output assets/app.css
    node styles/kit/kit-check.mjs templates src

# Fail when the committed stylesheet is not what the sources and the kit give, or markup strays outside the kit.
css-fresh: kit
    #!/usr/bin/env bash
    set -euo pipefail
    css=$(mktemp)
    trap 'rm -f "$css"' EXIT
    tailwindcss --optimize --input styles/app.css --output "$css"
    cmp -s "$css" assets/app.css || { echo 'assets/app.css is stale: run `just css`'; exit 1; }
    node styles/kit/kit-check.mjs templates src

# Point styles/kit at koan-kit-web's source, the version Cargo.lock names, so the stylesheet is compiled from the same
# kit release as the markup its components write. The link is gitignored.
kit:
    #!/usr/bin/env bash
    set -euo pipefail
    manifest=$(cargo metadata --format-version 1 --locked | python3 -c 'import json, sys; print(next(p["manifest_path"] for p in json.load(sys.stdin)["packages"] if p["name"] == "koan-kit-web"))')
    ln -sfn "$(dirname "$manifest")" styles/kit

# Format the Rust and the templates
fmt:
    cargo fmt --all
    askama-analyzer fmt

# Install the git hooks
hooks:
    lefthook install

# Write CHANGELOG.md from the fragments in changelog.d/. `--stdout` prints it instead.
changelog *args:
    python3 scripts/changelog.py {{args}}

# Open the release PR: `just release patch|minor|major`, or an exact version.
# Merging it publishes the image, the Pulumi package and the GitHub release; tags are never pushed by hand.
release version *args:
    scripts/release.sh {{version}} {{args}}
