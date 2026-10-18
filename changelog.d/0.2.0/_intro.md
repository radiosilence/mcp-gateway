Released 2026-07-26.

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
