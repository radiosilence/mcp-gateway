Released 2026-07-26.

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
