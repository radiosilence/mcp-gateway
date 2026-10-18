Released 2026-07-26.

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
