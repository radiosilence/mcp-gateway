This gateway stopped being Hydra's login provider and became an ordinary client
of it. Read the whole entry before deploying: it does not start on the old
configuration, and it cannot be rolled back past the migration without a
re-login.

### Changed

- **The dashboard signs you in through the issuer, not through GitHub.**
  `/login` now starts an authorization-code flow with PKCE against
  `HYDRA_ISSUER` and reads the identity out of the ID token. Which upstream
  vouches for a person, and who is allowed in at all, is the login provider's
  business — nothing here changes when either answer does.
- **`GH_CLIENT_ID`, `GH_CLIENT_SECRET` and `GH_ALLOWED` are gone**, replaced by
  `OIDC_CLIENT_ID` and `OIDC_CLIENT_SECRET`. A missing one is a refusal to
  start rather than a login that fails later. There is no GitHub OAuth app any
  more.
- **The allowlist is not duplicated here.** A token issues only to somebody the
  provider admitted, so arriving with a valid code *is* the check. A second copy
  was a second thing to keep in step and a second way to lock somebody out.
- `HYDRA_ADMIN_URL` is now used for token introspection only.
- The callback moved from `/auth/github/callback` to `/auth/callback`.

### Removed

- `GET /auth/login` and `GET /auth/consent` — the challenges Hydra used to
  delegate here. It delegates them elsewhere now, so these were unreachable.
- `POST /register` — the DCR proxy. Hydra advertises the provider's
  registration endpoint, so nothing discovered this one.
- `HydraAdmin::accept_login`, `get_consent` and `accept_consent`, whose only
  callers were the above.

### Migration

`0004_relying_party.sql` swaps `oauth_flows.login_challenge` for a PKCE
`verifier` and clears the table. Logins in flight at the moment of deploy are
lost, which costs a retry — the rows are one-shot and expire in ten minutes
anyway. Existing dashboard sessions are untouched: the `sessions` table did not
change, and only how a session is created did.
