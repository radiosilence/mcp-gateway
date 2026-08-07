# Changelog

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/);
versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.6.0] (2026-08-07)

Every dependency moved to its current release. Most of that is invisible, but
two changes below alter runtime behaviour — read those before deploying.

### Changed

- **`reqwest` 0.12 → 0.13 changes where TLS trust comes from.** 0.13 drops the
  compiled-in Mozilla root store entirely; its `rustls` feature now pulls
  `rustls-platform-verifier`, which reads the *system* trust store. The
  published image already ships a CA bundle for exactly this reason, so it is
  unaffected — but the bare binary now needs `/etc/ssl/certs/ca-certificates.crt`
  (or the platform equivalent) on any host that runs it, and will fail to build
  a client without one. `rustls-tls` was also renamed to `rustls`.
- **`sqlx` 0.8 → 0.9, moved from `tls-rustls-ring` to `tls-rustls-aws-lc-rs`.**
  reqwest's `rustls` feature hard-selects the aws-lc-rs provider, and two
  rustls crypto providers in one binary is how you get a process with no
  unambiguous default. Matching sqlx to it keeps exactly one.
- **`askama` 0.12 → 0.16 makes `{% call %}` a block**, so every macro call now
  carries a matching `{% endcall %}`. Rendered output was diffed against 0.12
  across every template branch and is byte-for-byte identical.
- **`rand` 0.8 → 0.10.** The `RngCore` trait is gone (the old `Rng` extension
  trait took its name); token generation uses `rand::fill` instead.
- **`chacha20poly1305` 0.10 → 0.11** (aead 0.6): nonces come from the `Generate`
  trait, and `Array::from_slice` is deprecated in favour of `TryFrom`. Two
  panics became errors as a result — a `TOKEN_ENC_KEY` of the wrong length and a
  stored blob with a malformed nonce are now reported, not a crash.
- `axum` 0.8.9, `tower-http` 0.6 → 0.7, `base64` 0.22 → 0.23, and the rest of
  the tree to current.

## [0.5.4] (2026-07-26)

### Changed

- **LTO and codegen-units tuned for gateway workload.** Fat LTO with a single
  codegen unit was the slowest possible build configuration, serializing
  whole-program optimization across the whole dependency tree. The gateway is
  I/O-bound — it proxies HTTP and talks to Postgres — and gains nothing
  measurable from that cost. Thin LTO with 16 codegen units instead.

## [0.5.3] (2026-07-26)

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

## [0.5.2] - 2026-07-26

### Fixed

- **A secret could never be shown as set.** The stored values were only fetched
  when an MCP had at least one visible field, because until now they were only
  needed to prefill one. Every MCP holding nothing but secrets — Fastmail, TfL —
  therefore rendered as though nothing were stored, which is precisely the case
  the marker exists for.

- **A backend that declares no check now reads as connected.** It was showing
  "configured", which implies a doubt there are no grounds for: nothing was
  asked, so nothing came back unanswered. TfL authenticates nothing and a key
  there only raises a rate limit, so there is nothing to be unsure about. A
  declared check that could not be completed still reads as configured, because
  that one genuinely is unknown.

- **A failed lookup is a sentence, not a clipped dropdown.** The reason came
  back inside a `<select>`, which cannot wrap, so it was cut off mid-word — and
  offering a control to choose from when there is nothing to choose is its own
  small lie.

- A check that cannot be completed now says so in the log. It returned unknown
  silently, which is unhelpful precisely when something is wrong.

## [0.5.1] - 2026-07-26

### Fixed

- **Only secrets are marked as set.** A visible field shows its own value, so
  saying it was set stated the same thing twice. A password box looks identical
  either way, which is the case worth answering.

## [0.5.0] - 2026-07-26

### Added

- **The dashboard says what it knows, not what it hopes.** "Connected" meant a
  credential was stored, which is a different claim from it working — a revoked
  token looked identical to a good one until something failed. An MCP can now
  declare a `verify` query, and the badge distinguishes not configured, stored
  but unconfirmed, confirmed, and refused.

  Both reporting styles are supported, because backends disagree: one that
  raises on bad auth needs only a query, one that answers calmly with a status
  names the values that mean working and refused. **An error is never treated as
  a rejection.** A server being unreachable says nothing about a password, and
  sending someone to rotate a working credential is worse than admitting
  ignorance — so anything unrecognised is unknown.

- **Fields say whether they hold anything.** A secret is never rendered back, so
  a set password and an empty one looked the same. They no longer do.

## [0.4.1] - 2026-07-26

### Fixed

- **Half the options timing was never actually added.** 0.3.1 said both the
  prefetch and the endpoint that serves what it misses were timed. Only the
  prefetch ever was — the edit adding the second one silently matched nothing,
  and compiling and passing tests does not notice an edit that did nothing. So
  the half that measures a slow backend, which was the reason for any of it, was
  missing. It is there now.

### Changed

- **One fetch behind both paths.** Asking a backend for a setting's choices —
  credentials, the query, reading the answer, working out which is stored — was
  written once for the page's prefetch and again for the endpoint htmx calls.
  They render the same control, so asking different questions was only ever a
  matter of time.

- **One definition of the status line.** The page rendered an empty one and a
  save replaced it, from two pieces of markup that had already drifted once.

## [0.4.0] - 2026-07-26

### Changed

- **Saving and disconnecting no longer reload the page.** Both post over htmx
  and are answered with the section they changed, so the badge, the timestamp
  and the settings all update together — and a wrong password corrected in the
  form immediately shows the choices the new one can see, which previously took
  a round trip through a redirect.

  The section became a template of its own for this, rendered by the page and
  again on its own, so the two cannot describe the same MCP differently. What a
  mutation wants to say is carried on the section rather than in a flash
  parameter, which no longer has a page load to survive.

- **Disconnect confirms via `hx-confirm`.** The handler that did it listened for
  form submission, and htmx binds that on the element — it would have fired the
  request before the confirmation ran. Worth stating plainly because it would
  have looked fine and quietly stopped asking.

### Internal

- Being signed in is an extractor rather than four hand-written guards. The
  failure mode for a fifth handler that forgot one is an unauthenticated
  endpoint on a credential vault, which is not a thing to leave to memory. Two
  of them, because a browser wants the login page and an htmx fragment wants an
  error — a redirect there would be followed and swapped into the page.
- `dashboard.rs` was 890 lines doing three jobs; it is now handlers, the view
  and its constructor, and the options fetching, with each module's tests beside
  the code they cover.

## [0.3.1] - 2026-07-26

### Added

- **Timing on both halves of the options fetch.** The 600ms render budget was a
  guess, and a guess with nothing measuring it stays one — worse, it failed
  quietly, since a page that came up short looked no different to one with
  nothing to fetch.

  Whatever beats the budget is logged as it arrives; whatever misses is timed by
  the endpoint that then serves it, which has to fetch the thing anyway. Both
  halves are covered without keeping abandoned work alive purely to measure it,
  and a render that came up short reports how many of how many. Enough to tell
  a budget that is too tight from a backend that is simply slow.

## [0.3.0] - 2026-07-26

### Changed

- **The page arrives complete.** Settings whose choices come from a backend are
  fetched while the dashboard renders — all of them at once, with a 600ms budget
  — so a connected account shows its real dropdown immediately rather than a
  placeholder that fills in. Anything that misses the budget still loads over
  htmx afterwards, which is also what happens when a backend is refusing, so the
  slow path is the same path rather than a second one that only runs when
  something is wrong.

  The budget is the point: these are network calls to somebody else's server on
  the way to rendering, and the dashboard is what you would use to fix the
  credential that is making them slow. It waits a little for a nicer page and
  never waits long.

- **One definition of a form control.** The placeholder, the control that
  replaces it and the credential inputs all carried the same class list written
  out three times, which is how they came to disagree: a class added to one and
  not the others changed the box model and the swap moved the page. They share a
  macro now.

- **"updated 3 hours ago" instead of a timestamp.** The exact time was rendered
  as RFC 3339 and rewritten by script into the reader's locale, which meant an
  ISO string visibly sitting there until that ran. No request header carries a
  timezone — `Accept-Language` gives a locale, nothing gives a zone — so the
  server cannot format an exact local time at all, and elapsed time sidesteps
  the question. The precise value stays on the element for anything that wants
  it, and there is no longer any script involved.

## [0.2.1] - 2026-07-26

### Fixed

- **A settings dropdown flashed a raw identifier before it resolved.** An
  option is keyed by its id rather than its label, because labels are not
  guaranteed unique, so a stored value showed as a bare identifier for as long
  as the choices took to arrive.

  It is a skeleton now: a `<select>` carrying the control's own classes, pulsing
  with no text and no arrow, replaced outright once the choices land. The same
  element and the same classes as the thing that replaces it, so the two cannot
  differ in height and nothing reflows. Rendering it as the initial state rather
  than styling a request-in-flight class also means it is a skeleton from first
  paint rather than from whenever the request starts, which is the gap it exists
  to cover.

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
