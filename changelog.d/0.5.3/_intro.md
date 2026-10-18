Released 2026-07-26.

### Changed

- **Image is now a single-stage `scratch` copy, not a `debian:bookworm-slim`
  build.** CI compiles the static musl binary and the Dockerfile only copies
  it in, plus the CA bundle `reqwest`/`rustls-platform-verifier` need for the
  outbound HTTPS calls this gateway makes (Hydra, GitHub OAuth, proxied MCP
  backends) — sourced from `gcr.io/distroless/static`, since there is no
  package manager to install `ca-certificates` with anymore. No templates,
  assets or migrations to copy in: they're embedded at compile time already.
  Aligns with the same change already shipped in nano-web, tfl-mcp,
  mainlynorfolk-mcp and caldav-cli.
- CI restructured to match: one `build-image` job compiles the musl binary
  and builds the image on the same runner per architecture (matrix
  amd64/arm64), always building on PRs and only pushing on `main`. This
  repo ships no binary artifacts, so there is no `build-binaries` job and
  nothing is attached to the GitHub release — the image remains the only
  deliverable. Published tag scheme (`main`, `sha-<short>`, `vX.Y.Z`/`vX.Y`/
  `vX`, `latest`) is unchanged.
