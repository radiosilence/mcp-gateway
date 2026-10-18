### Changed

- **Fastmail declares three credential fields instead of one token.** Mail runs
  on an API token, but contacts go over CardDAV — a separate protocol that
  rejects API tokens — so a token-only registry entry could never reach them,
  and the backend reported contacts as permanently unconfigured however the user
  filled the form in. The username and app password are `required: false`:
  without them mail works and contact search doesn't, which
  [`fastmail-cli`](https://github.com/radiosilence/fastmail-cli) reports as
  `session { carddavConfigured }` rather than failing a query halfway through
  one.

  No migration. The shorthand normalised to a field with id `token`, which is
  what the entry now declares explicitly, under the same `X-Fastmail-Token`
  header — stored credentials keep working and connected users stay connected.
  Registry-only: `fields` needed no gateway change, and `credential_header`
  remains for backends with a single secret.
