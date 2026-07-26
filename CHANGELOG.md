# Changelog

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.2.1] - 2026-07-26

### Fixed

- **A settings dropdown flashed a raw identifier before it resolved.** Calendars
  are keyed by id rather than name, deliberately — names are not unique — so the
  stored value rendered as a bare UUID for as long as the choices took to
  arrive. It now reads as a skeleton instead: the placeholder says "Loading…",
  and while the request is in flight the control pulses with its text
  transparent, so it is a shape rather than a word one has to read and discard.

  The skeleton is a `<select>` with the control's own classes, so the two cannot
  differ in height and nothing reflows when the choices land. It is replaced
  outright rather than filled in, which also means the placeholder is a skeleton
  from first paint — keyed off a request-in-flight class it would have shown
  plain text until htmx started, which is exactly the moment it is meant to
  cover.

## [0.2.0] - 2026-07-26

### Changed

- **The settings dropdown is htmx rather than hand-written fetch code.** The
  server already owned the markup and already parsed the backend's reply, so the
  script was doing the one part that did not need to be in a browser: deciding
  which choices to offer, which to mark as the account's own, and which to
  preselect. That now happens in Rust beside the parsing it depends on, and is
  tested. `assets/app.js` drops from 73 lines to 29 — what is left is clipboard
  and locale formatting, which are browser APIs and belong there.

  The two endpoints behind it return HTML fragments instead of JSON, so they are
  now specific to this page. That is the trade: they were reusable and are not
  any more, in exchange for the shaping being somewhere it can be read and
  tested.

- **htmx 4.0.0-beta6 is vendored and imported as a module.** Vendored for the
  same reason as the stylesheet — a CDN script on this page can read credentials
  as they are typed, and our own policy would block it anyway. Imported rather
  than script-tagged so there is one entry point and no global; by absolute path
  rather than by name, because a bare specifier needs an import map and those
  are inline script the policy also refuses.

  A beta deliberately: v4 keeps every attribute this uses (`hx-patch`,
  `hx-target` with `find`/`next`, the `change` default on a select, a select's
  value in the request), so the choice is between a beta and porting later. What
  it does *not* keep is the v2 response-header protocol — `HX-Retarget`,
  `HX-Reswap`, `HX-Redirect` and the rest are gone — which is why nothing here
  depends on one.

  No `hx-on:` attributes anywhere: those build functions at runtime, which the
  policy refuses, and quietly adding `unsafe-eval` to make one work would undo
  most of what it is for.

## [0.1.2] - 2026-07-26

### Security

- **The dashboard no longer loads anything from a CDN.** It pulled Tailwind from
  `cdn.tailwindcss.com` — a third-party script with full DOM access, on pages
  behind the login of a service that holds credentials and into which they are
  typed in plaintext. A compromise of that CDN could read them as they were
  entered. The stylesheet is now generated at build time and embedded in the
  binary along with the script, so the page fetches nothing but itself.
- **A Content-Security-Policy, now that one is possible.** `default-src 'self'`
  with no `unsafe-inline` anywhere, plus `frame-ancestors 'none'`, `base-uri
  'none'`, `object-src 'none'`, `nosniff` and `Referrer-Policy: no-referrer`. It
  would have been unenforceable while the page pulled a CDN script and carried
  an inline handler, which is why it lands with them rather than after.
- **The one inline event handler is gone**, moved to a `data-confirm` attribute.
  It interpolated a registry name into a JavaScript string literal, so a name
  containing a quote could break out of it. Removing it also leaves no inline
  script on the page, which is what a strict Content-Security-Policy needs.

## [0.1.1] - 2026-07-26

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

## [0.1.0] - 2026-07-26

First release. The gateway ran unversioned until now, deployed by commit sha, so
this entry describes the shape it arrived at rather than replaying how it got
there.

### Added

- **One OAuth login in front of many MCP servers.** Claude reaches
  `https://<host>/<id>` for each MCP; the gateway is the OAuth resource server,
  introspects the bearer at Hydra, looks up that user's credential for that MCP,
  and reverse-proxies to a backend pod with the secret injected as a header.
  Backends hold no auth of their own and link no gateway code — the alternative
  was every MCP server growing its own OAuth, which is the same work N times.
- **A registry that is configuration, not code.** `MCP_REGISTRY` carries the
  whole registry as a YAML (or JSON) document: which MCPs exist, what each is
  called, and what it needs from the user. Adding one is an entry, not a deploy
  of new code, and the gateway ships no registry of its own — it belongs to
  whoever runs it.
- **Credentials that are not all bearer tokens.** An MCP declares
  `credential_header` for the one-token case or `fields` for anything else;
  CalDAV needs a username, an app password and a server URL, each injected into
  its own header. Fields can be optional, carry defaults, and be marked
  non-secret so the dashboard shows them for editing.
- **Public MCPs.** `public: true` fronts something that needs no credentials at
  all, and shows no connect form. Opt-in rather than inferred from an absent
  credential block, because declaring none is far more often a typo than a
  decision.
- **Settings sourced from the backend.** A field can carry an `options_query`,
  run against the backend's own GraphQL endpoint with the user's credentials, so
  the dashboard offers a real list — your actual calendars — instead of asking
  you to paste an identifier. A matching `sync_mutation` tells the backend what
  was picked, best-effort: the gateway's stored value is what the proxy injects
  and stays authoritative if the backend refuses.
- **A dashboard** to connect, edit and disconnect each MCP, with per-client
  instructions for Claude and Claude Code.

### Security

- **Login allowlist** (`GH_ALLOWED`). Dynamic client registration is public
  because Claude requires it and consent is auto-granted, so the allowlist is
  the thing standing between a stranger who registered a client and a token.
- **Opaque access tokens**, introspected per request. No JWT reaches a client
  and tokens are revocable at Hydra.
- **Envelope encryption at rest** (XChaCha20-Poly1305). Stored credentials are
  ciphertext; `TOKEN_ENC_KEY` is the only thing that opens them, and plaintext
  never lands in the database.
- **Server-side sessions.** Cookies carry an opaque id and nothing else, so no
  state is forgeable by a client and a session dies when its row does.
- Registry ids that would shadow a gateway route are rejected at startup, as are
  header names that could not be sent — both would otherwise fail deep in the
  proxy at request time.
