# mcp-gateway

> Repo is still named `fastmail-mcp-service` (Fastmail was the first backend);
> the code is now a generic gateway. Rename happens when it folds into jaritanet.

An OAuth-fronted gateway for self-hosted MCP servers. Add any of your MCPs to
Claude (Desktop / mobile / web) as custom connectors without each one needing
its own OAuth — the gateway authenticates the user once and injects each MCP's
credential.

## Why this exists

MCP servers like [`fastmail-cli`](https://github.com/radiosilence/fastmail-cli)
are single-tenant (one key, local stdio), but Claude's remote connectors need an
OAuth-authenticated HTTPS endpoint. Rather than bolt OAuth + key storage onto
every MCP, the gateway centralizes it:

1. Authenticate the **user** via OAuth (Ory Hydra as the AS, GitHub as the
   upstream identity — no passwords stored).
2. Map that identity to a per-MCP key the user pastes into a dashboard, stored
   encrypted.
3. Proxy `/mcp/<id>` to that MCP's backend, injecting the key per request.

The OAuth token proves *who you are*; it is never the MCP's key.

## Architecture (gateway + backends)

```
Claude ──OAuth──► Hydra (AS: DCR via our /register proxy, PKCE, tokens)
                    ▲ login/consent delegated back to the gateway
Claude ──Bearer─► gateway (axum, N pods)
                    ├ introspect opaque bearer at Hydra → sub
                    ├ dashboard: manage a key per MCP
                    ├ Postgres: (sub, mcp_id) → enc(key)   (XChaCha20-Poly1305)
                    └ /mcp/<id> ──inject <MCP's header>──► backend MCP pod
                                                             e.g. fastmail-cli mcp --http
```

Each MCP is a backend pod the gateway proxies to (Model B); the gateway links no
MCP code. Backends stay dumb: key in → work out. Adding an MCP is an entry in
`mcps.json`, not code. Hydra + Postgres are separate deployments; only Hydra
serves OAuth endpoints (the gateway publishes protected-resource metadata
pointing at it, and a `/register` DCR proxy).

State is disposable: lose the DB and users just re-paste their keys.

## Local development

```sh
cp .env.example .env
# create a GitHub OAuth app (callback http://localhost:8080/auth/github/callback)
# and fill GH_CLIENT_ID / GH_CLIENT_SECRET; set a real TOKEN_ENC_KEY:
#   openssl rand -base64 32
docker compose up --build
```

You can exercise the whole browser flow (login, set token, test connection) at
`http://localhost:8080` directly — no HTTPS needed for a local browser.

### Testing the Claude connector (Cloudflare tunnel)

Claude's connector is fetched by Anthropic's servers, not your machine, so
`localhost` is unreachable — and **both** the service and Hydra must be public
(Claude talks to each directly). One tunnel, two hostnames does it:

Hostnames default to `mcp.radiosilence.dev` / `auth.radiosilence.dev` (dev;
prod is `mcp.blit.cc` / `auth.blit.cc`). Set `GATEWAY_HOST` / `AUTH_HOST` in
`mise.toml` or per-invocation for your own domain. Then:

1. Point the GitHub OAuth app's callback at
   `https://<GATEWAY_HOST>/auth/github/callback`.
2. `mise run tunnel` — provisions the tunnel + DNS (one-time browser
   `cloudflared tunnel login` if not already), writes `cloudflared/`, and brings
   the stack up with the tunnel URLs wired in automatically.
3. `mise run verify` — checks the OAuth discovery chain over the tunnel.
4. Add `https://<GATEWAY_HOST>/<id>` as a custom connector in Claude — e.g.
   `https://mcp.radiosilence.dev/fastmail`.

Host login (`cert.pem`) is only for provisioning; the container authenticates
the *run* with the per-tunnel `creds.json`. The service and Hydra stay plain
HTTP inside the compose network — Cloudflare terminates TLS at the edge (the
role Traefik plays in production).

Local (no tunnel): `mise run up` → `http://localhost:8080`.

## Configuration

All via env (see `.env.example`). Notable:

| var | meaning |
|-----|---------|
| `PUBLIC_URL` | browser/Claude-facing base of the gateway |
| `TOKEN_ENC_KEY` | 32-byte base64 key; the only thing protecting stored credentials |
| `HYDRA_ISSUER` | browser/Claude-facing Hydra URL; advertised in protected-resource metadata |
| `HYDRA_ADMIN_URL` | Hydra admin API (introspection, login/consent, DCR client-create) — cluster-internal only |
| `MCP_REGISTRY` | path to the MCP registry JSON (default `mcps.json`) |

### MCP registry (`mcps.json`)

Each backend MCP is one entry — adding an MCP is config, not code:

```json
[
  {
    "id": "fastmail",
    "name": "Fastmail",
    "backend": "http://fastmail-mcp:8080/mcp",
    "credential_header": "X-Fastmail-Token",
    "key_help_url": "https://app.fastmail.com/settings/security/tokens",
    "key_hint": "fmu1-…"
  }
]
```

`/fastmail` proxies to `backend`, injecting the user's stored key as
`credential_header`. (Ids that would shadow a gateway route — `register`,
`auth`, `login`, `logout`, `dashboard`, `healthz`, `.well-known` — are rejected
at startup.)

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
- **DCR runs through our `/register` proxy** (Hydra advertises it; we create the
  client via Hydra admin and return a Claude-valid response). Hydra itself runs
  `--dev` in compose — **not safe for prod**; drop `--dev` and configure real
  TLS/secrets there.
- **Encryption key rotation** is not implemented — rotating `TOKEN_ENC_KEY`
  currently orphans stored tokens (users re-paste). Fine given disposable state,
  but decide intentionally.
- **DCR is public + consent is auto-granted.** Claude requires Dynamic Client
  Registration (it auto-registers), so DCR must stay enabled — but public DCR +
  auto-consent means anyone could register a client and phish a token-holder
  into authorizing it. The control against this is a **GitHub login
  allowlist** (`GH_ALLOWED`, see [`config.rs`](src/config.rs)): a
  comma-separated list of GitHub logins permitted to authenticate, checked in
  the GitHub OAuth callback ([`auth/routes.rs`](src/auth/routes.rs)) before a
  session or Hydra login is granted — a registered client is useless without a
  token, and tokens only issue to allowlisted users. **Leaving `GH_ALLOWED`
  unset or empty allows *any* GitHub account to authenticate**; the gateway
  logs a loud warning at boot in that case ([`main.rs`](src/main.rs)) but does
  not refuse to start. Set `GH_ALLOWED` before a public deploy. Add a real
  consent screen too if you ever go multi-user.
- Custodial risk, amplified: the gateway holds keys for *every* MCP and user
  (e.g. full-mailbox Fastmail tokens). Keep `TOKEN_ENC_KEY` in a real secret
  store with tight RBAC; never in the image or git.
