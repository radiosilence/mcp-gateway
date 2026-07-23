# fastmail-mcp-service

Hosted, OAuth-fronted MCP server for Fastmail. Lets you add Fastmail to Claude
(Desktop / mobile / web) as a custom connector, without Fastmail OAuth.

It wraps [`fastmail-cli`](https://github.com/radiosilence/fastmail-cli)'s MCP
server (linked as a library), puts an OAuth layer in front, and maps each
authenticated user to their own encrypted Fastmail API token.

## Why this exists

`fastmail-cli mcp` is single-tenant: one token, local stdio. Claude's remote
connectors require an OAuth-authenticated HTTPS endpoint. Fastmail has no OAuth
for third-party apps (short of an approval process), only all-or-nothing API
tokens. So instead of using Fastmail's (nonexistent) OAuth, we:

1. Authenticate the **user** to us via OAuth (Ory Hydra as the AS, GitHub as the
   upstream identity — you never store passwords).
2. Map that identity to a Fastmail API token the user pastes into a dashboard,
   stored encrypted.
3. Serve MCP over HTTP, injecting that token per request into the core.

The OAuth token proves *who you are*; it is never the Fastmail token.

## Architecture (Model A)

```
Claude ──OAuth──► Hydra (AS: DCR, PKCE, tokens)   ← login/consent delegated back to us
Claude ──Bearer─► this service (axum, N pods)
                    ├ validate JWT vs Hydra JWKS → sub
                    ├ dashboard: set/update/delete Fastmail token
                    ├ Postgres: sub → enc(fastmail_token)   (XChaCha20-Poly1305)
                    └ /mcp → fastmail-cli core, X-Fastmail-Token injected → JMAP
```

One process does auth + token store + dashboard + MCP (links the core as a
library). Hydra and Postgres are separate deployments. Only Hydra ever serves
OAuth endpoints — this service just publishes
`/.well-known/oauth-protected-resource` pointing at it.

State is disposable: lose the DB and users just re-paste their token.

## Local development

```sh
cp .env.example .env
# create a GitHub OAuth app (callback http://localhost:8080/auth/github/callback)
# and fill GITHUB_CLIENT_ID / GITHUB_CLIENT_SECRET; set a real TOKEN_ENC_KEY:
#   openssl rand -base64 32
docker compose up --build
```

Then add `http://localhost:8080/mcp` as a custom connector in Claude. Sign in,
set your Fastmail token in the dashboard, done.

## Configuration

All via env (see `.env.example`). Notable:

| var | meaning |
|-----|---------|
| `PUBLIC_URL` | browser-facing base of this service |
| `TOKEN_ENC_KEY` | 32-byte base64 key; the only thing protecting stored tokens |
| `HYDRA_PUBLIC_URL` | where *this service* fetches Hydra JWKS (may be internal) |
| `HYDRA_ISSUER` | token `iss` / browser-facing Hydra URL (defaults to `HYDRA_PUBLIC_URL`) |
| `HYDRA_ADMIN_URL` | Hydra admin API — cluster-internal only |

## Deploy

`deploy/k8s/` holds the manifests (namespace, Postgres, Hydra + migrate Job,
the service, ingress with cert-manager TLS). `deploy/terraform/` is a child
module for jaritanet: it manages the namespace + Secret (sourced from the secret
backend) and applies the manifests. A deploy is a PR that runs `tf plan`.

The Hydra admin Service is ClusterIP-only and must never be exposed.

## ⚠️ Security review items (before this is internet-facing)

These are deliberately stubbed/simplified in this first cut — see inline
`SECURITY REVIEW` notes:

- **JWT audience not enforced** (`src/auth/jwt.rs`). Pin a resource audience and
  validate `aud`, or a token minted for another resource could be replayed.
- **Hydra DCR / public client registration** must be explicitly configured for
  production (the compose file runs `--dev`, which is not safe for prod).
- **Encryption key rotation** is not implemented — rotating `TOKEN_ENC_KEY`
  currently orphans stored tokens (users re-paste). Fine given disposable state,
  but decide intentionally.
- **Consent is auto-granted** (single-tenant assumption). If you ever allow
  arbitrary signups, add a real consent screen and lock down who may register.
- Custodial risk: you hold users' full-mailbox Fastmail tokens. Keep the enc key
  in a real secret store with tight RBAC; never in the image or git.
