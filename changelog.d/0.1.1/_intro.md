Released 2026-07-26.

### Fixed

- **The dashboard printed a raw Rust timestamp.** "updated 2026-07-24
  12:03:46.848403 +00:00:00" was `OffsetDateTime`'s `Display`, which is a debug
  format that happened to reach a user. The server now emits RFC 3339 into a
  `<time datetime>` and the browser renders it in the viewer's own locale and
  timezone; without scripting it degrades to the RFC 3339 string, which is at
  least a date a human can read.
- **An MCP needing no credentials claimed to have some.** A public entry was
  rendered as "Credentials set · updated ." — the trailing full stop being an
  empty timestamp — because having nothing to store counts as being connected.
  It now says so plainly.

### Changed

- **This repo no longer deploys anything.** CI used to bump an image tag in the
  deployment that runs the gateway, which meant holding a write credential for
  someone else's repository — the implementation reaching into an instance.
  Deployments now watch releases and decide when to move.
