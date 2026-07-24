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
                    ├ introspect opaque bearer at Hydra → sub
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

You can exercise the whole browser flow (login, set token, test connection) at
`http://localhost:8080` directly — no HTTPS needed for a local browser.

### Testing the Claude connector (Cloudflare tunnel)

Claude's connector is fetched by Anthropic's servers, not your machine, so
`localhost` is unreachable — and **both** the service and Hydra must be public
(Claude talks to each directly). One tunnel, two hostnames does it:

1. In **Cloudflare Zero Trust → Tunnels**, create a tunnel and add two public
   hostnames pointing at the in-network targets:
   - `fastmail-dev.<your-domain>` → `http://service:8080`
   - `auth-dev.<your-domain>` → `http://hydra:4444`
2. Copy the tunnel token into `.env` as `TUNNEL_TOKEN`, and set:
   - `PUBLIC_URL=https://fastmail-dev.<your-domain>`
   - `HYDRA_ISSUER=https://auth-dev.<your-domain>`
3. Update the GitHub OAuth app's callback to
   `https://fastmail-dev.<your-domain>/auth/github/callback`.
4. `docker compose --profile tunnel up`
5. Add `https://fastmail-dev.<your-domain>/mcp` as a custom connector in Claude.

The service and Hydra stay plain HTTP inside the compose network; Cloudflare
terminates TLS at the edge (the same role Traefik plays in production).

## Configuration

All via env (see `.env.example`). Notable:

| var | meaning |
|-----|---------|
| `PUBLIC_URL` | browser-facing base of this service |
| `TOKEN_ENC_KEY` | 32-byte base64 key; the only thing protecting stored tokens |
| `HYDRA_ISSUER` | browser/Claude-facing Hydra URL; advertised in protected-resource metadata |
| `HYDRA_ADMIN_URL` | Hydra admin API (introspection + login/consent) — cluster-internal only |

## Deploy

`docker-compose.yml` is the **source of truth for the topology**; production is
that same shape translated to jaritanet's Pulumi. This repo holds no cluster
manifests by design. See [`DEPLOY.md`](DEPLOY.md) for the prod deltas (domains,
TLS, admin-not-exposed, secrets from the backend, opaque tokens).

## ⚠️ Security review items (before this is internet-facing)

These are deliberately stubbed/simplified in this first cut:

- **Access tokens are opaque + introspected** at Hydra (no JWT reaches any
  client; tokens are revocable). Introspection is per-request — add a short TTL
  cache if load warrants; note that caching trades away instant revocation.
- **Hydra DCR / public client registration** must be explicitly configured for
  production (the compose file runs `--dev`, which is not safe for prod).
- **Encryption key rotation** is not implemented — rotating `TOKEN_ENC_KEY`
  currently orphans stored tokens (users re-paste). Fine given disposable state,
  but decide intentionally.
- **Consent is auto-granted** (single-tenant assumption). If you ever allow
  arbitrary signups, add a real consent screen and lock down who may register.
- Custodial risk: you hold users' full-mailbox Fastmail tokens. Keep the enc key
  in a real secret store with tight RBAC; never in the image or git.
