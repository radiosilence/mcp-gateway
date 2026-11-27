# mcp-gateway — an OAuth-fronted gateway for self-hosted MCP servers

# What CI checks: format (Rust and templates), lint with warnings as errors,
# tests, the workflows.
check:
    cargo fmt --all -- --check
    askama-analyzer fmt --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test --locked --all-features
    actionlint

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
