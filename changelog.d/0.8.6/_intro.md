### Changed

- **Spacing across a service's card.** A caption sat 4px under its heading and
  a label 4px above its control, which is close enough to touching to read as
  one thing. An open disclosure had 16px under its content and 10px above it,
  so everything sat high in the box. What you can *do* to a connection — edit
  the credentials, disconnect — is now set apart from what it *is*, and grouped
  rather than evenly spaced with it.

### Added

- **`mise run preview`.** Renders the dashboard to a static file from fixtures
  covering every state a section has, so how it looks can be checked without
  standing up Hydra and the login provider. htmx is dropped along with the
  stylesheet link, which is what keeps the skeleton a skeleton and the badge
  pending — the two states hardest to catch in the act, and the reason the
  loading field went so long looking like an empty one.
