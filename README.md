# mcp-gateway

![The dashboard: each MCP the gateway fronts, its connection state, and how to add it to Claude](docs/dashboard.png)

An OAuth-fronted gateway for self-hosted MCP servers. Add any of your MCPs to
Claude (Desktop / mobile / web) as custom connectors without each one needing
its own OAuth — the gateway authenticates the user once and injects each MCP's
credential.

## Why this exists

MCP servers like [`fastmail-cli`](https://github.com/radiosilence/fastmail-cli)
and [`caldav-cli`](https://github.com/radiosilence/caldav-cli) are single-tenant
(one key, local stdio), but Claude's remote connectors need an
OAuth-authenticated HTTPS endpoint. Rather than bolt OAuth + key storage onto
every MCP, the gateway centralizes it:

1. Authenticate the **user** via OAuth (Ory Hydra as the AS, GitHub as the
   upstream identity — no passwords stored).
2. Map that identity to the per-MCP credentials the user enters in a dashboard,
   stored encrypted.
3. Proxy `/{id}` to that MCP's backend, injecting those credentials per request.

The OAuth token proves *who you are*; it is never the MCP's key.

Three MCPs ship in the registry today: **Fastmail** (mail and contacts, one API
token), **Calendar** (CalDAV — iCloud by default, username + app password +
server URL), and **Folk** (the Mainly Norfolk English folk archive — no
credentials at all).

Folk is the first *public* MCP: it reads a public website, so there is nothing
to authenticate to and nothing to store. It appears on the dashboard already
connected, with a connector URL and no form. Public is opt-in via
`"public": true` in the registry rather than inferred from an absent
`credential_header` — an MCP that declares no credentials is far more often a
typo than a decision, and the gateway should not quietly front an unauthenticated
backend because someone missed a line of JSON.

The OAuth login still applies: it establishes who the user is before the
gateway proxies anything. Public means *this backend needs no key of its own*,
not that the route is open.

## Architecture

```mermaid
flowchart LR
  C["Claude<br/>Desktop · Code · web"]

  subgraph ns["cluster namespace"]
    G["gateway<br/>axum, N pods"]
    H["Hydra<br/>OAuth AS"]
    A["auth<br/>login + consent provider"]
    DB[("Postgres<br/>encrypted creds")]
    B["backend MCP pods<br/>fastmail-cli · caldav-cli"]
  end

  C -->|"① OAuth: PKCE, DCR"| H
  H -.->|"login + consent<br/>delegated out"| A
  G -.->|"dashboard login<br/>(ordinary client)"| H
  C -->|"② Bearer token"| G
  G -->|"introspect → sub"| H
  G -->|"lookup enc creds"| DB
  G -->|"③ proxy /{id}, inject<br/>MCP's credential headers"| B
```

- **Hydra** is the only thing serving OAuth. The gateway publishes
  protected-resource metadata pointing at it, and Claude discovers Dynamic
  Client Registration from there — at the login provider, not here.
- **The login provider** answers the login and consent challenges Hydra
  delegates, and decides who may sign in at all. It is a separate service
  because Hydra takes one for the whole authorization server: while it lived
  here, this gateway was the login screen for every client of that Hydra, and
  an unhealthy replica of it was a failed login somewhere unrelated. This
  gateway is now an ordinary client of it, like any other.
- **The gateway** validates the opaque bearer by introspection (no JWT ever
  reaches a client), looks up the caller's per-MCP key, and reverse-proxies
  `/{id}` to that MCP's backend pod, injecting the key as the MCP's own header.
- **Backends stay dumb**: key in → work out. They link no gateway code and hold
  no auth of their own. Adding an MCP is one entry in the registry, not code
  (Model B).
- **State is disposable**: lose Postgres and users just re-paste their keys
  (encrypted at rest with XChaCha20-Poly1305). Hydra + Postgres are separate
  deployments.

## Local development

```sh
cp .env.example .env
# register this gateway at the login provider (redirect
# http://localhost:8080/auth/callback), fill OIDC_CLIENT_ID / OIDC_CLIENT_SECRET
# and point AUTH_URL at it; set a real TOKEN_ENC_KEY:
#   openssl rand -base64 32
docker compose up --build
```

You can exercise the whole browser flow (login, set token, test connection) at
`http://localhost:8080` directly — no HTTPS needed for a local browser.

The page loads nothing from the internet: the stylesheet and script are embedded
in the binary. The stylesheet is generated from the templates and committed, so
**after changing a template or `assets/app.js`, run `mise run css`** and commit
the result — CI rebuilds it and fails if it differs. Tailwind comes from mise,
so there is no node toolchain to install.

### Testing the Claude connector (Cloudflare tunnel)

Claude's connector is fetched by Anthropic's servers, not your machine, so
`localhost` is unreachable — and **both** the service and Hydra must be public
(Claude talks to each directly). One tunnel, two hostnames does it:

Set `GATEWAY_HOST` / `AUTH_HOST` (in `mise.toml` or per-invocation) to two
hostnames on a domain you control — one for the gateway, one for Hydra. Then:

1. Register this gateway as a client at the login provider, with redirect URI
   `https://<GATEWAY_HOST>/auth/callback`.
2. `mise run tunnel` — provisions the tunnel + DNS (one-time browser
   `cloudflared tunnel login` if not already), writes `cloudflared/`, and brings
   the stack up with the tunnel URLs wired in automatically.
3. `mise run verify` — checks the OAuth discovery chain over the tunnel.
4. Add `https://<GATEWAY_HOST>/{id}` as a custom connector in Claude — e.g.
   `https://<GATEWAY_HOST>/fastmail` or `https://<GATEWAY_HOST>/caldav`.

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
| `HYDRA_ISSUER` | browser/Claude-facing Hydra URL; advertised in protected-resource metadata, and where a dashboard login is sent |
| `HYDRA_ADMIN_URL` | Hydra admin API — introspection only — cluster-internal |
| `OIDC_CLIENT_ID` / `OIDC_CLIENT_SECRET` | this gateway's own registration at the issuer; the login provider generates them |
| `MCP_REGISTRY` | the MCP registry document itself (YAML or JSON) — required |

### MCP registry (`MCP_REGISTRY`)

Each backend MCP is one entry — adding an MCP is config, not code:

```yaml
- id: fastmail
  name: Fastmail
  backend: http://fastmail-mcp:8080/mcp
  key_help_url: https://app.fastmail.com/settings/security/tokens
  fields:
    - id: token
      label: API token
      header: X-Fastmail-Token
      hint: fmu1-…
```

The gateway ships no registry of its own. It arrives whole in the environment
rather than as a path to a file, because the registry describes a *deployment* —
which MCPs it fronts and where they live — and whoever deploys the gateway
already holds that as config. A file would be a second copy to keep in sync, and
a mounted ConfigMap edited under a running pod is invisible to it; env is part
of the pod spec, so changing the registry rolls the deployment.

JSON is accepted too, since every JSON document is valid YAML.
`docker-compose.yml` is the worked example: the registry sits in the `gateway`
service's environment, beside the backend services it names.

`/fastmail` proxies to `backend`, injecting each stored value into the header
its field names. (Ids that would shadow a gateway route — `register`,
`auth`, `login`, `logout`, `dashboard`, `healthz`, `.well-known` — are rejected
at startup.)

#### Saying whether the credentials actually work

A stored credential is not a working one. An MCP can declare how to ask:

```yaml
verify:
  query: "{ viewer { status } }"
  path: viewer.status            # omit: answering without error is the answer
  ok: CONNECTED                  # omit: any truthy value passes
  rejected: INVALID_CREDENTIALS  # omit: nothing is ever called rejected
```

The dashboard then says which of four things is true — not configured, stored
but unconfirmed, confirmed, or refused — instead of showing "connected" for
anything it happens to hold.

Backends report bad auth differently, so both shapes work. One that raises needs
only `query`; one that answers calmly with a status names the values. **An error
never counts as a rejection** — only an explicit `rejected` match does, because
a server being unreachable is not evidence about a password, and telling someone
to rotate a working credential is worse than saying nothing.

Omit `verify` entirely and the gateway makes no claim, which is the right answer
for a backend that authenticates nothing.

#### MCPs that need more than one value

`credential_header: X-Some-Token` stays available as shorthand for a backend
with exactly one secret, and is normalised into a one-entry `fields` at load, so
nothing downstream knows which form was used.

Plenty of backends don't fit it.
[`caldav-cli`](https://github.com/radiosilence/caldav-cli) authenticates with a
username *and* an app password, against a server URL that differs per provider.
Fastmail needs three: an API token for mail, plus a username and app password
for contacts, because CardDAV is a separate protocol that rejects API tokens.
Such an MCP declares `fields`, and each field is injected into its own header:

```yaml
- id: caldav
  name: Calendar (CalDAV)
  backend: http://caldav-mcp:8080/mcp
  graphql: http://caldav-mcp:8080/graphql
  key_help_url: https://appleid.apple.com
  fields:
    - id: username
      label: Apple ID / username
      header: X-CalDAV-Username
      secret: false
      hint: you@icloud.com
    - id: password
      label: App-specific password
      header: X-CalDAV-Password
      hint: abcd-efgh-ijkl-mnop
    - id: url
      label: CalDAV server
      header: X-CalDAV-Url
      secret: false
      default: https://caldav.icloud.com
      required: false
    - id: calendar
      label: Default calendar for new events
      header: X-CalDAV-Calendar
      secret: false
      required: false
      options_query: "{ options: calendars(first: 100) { nodes { value: id label: name disabled: readOnly isDefault } } }"
      sync_mutation: "mutation($value: String!) { setDefaultCalendar(id: $value) { success error } }"
```

| key | meaning |
|-----|---------|
| `id` | form field name and storage key |
| `label` | shown in the dashboard |
| `header` | header the backend reads this value from |
| `secret` | default `true`; secrets render as password inputs and are never echoed back. Non-secret values (a server URL) are shown so they can be edited in place |
| `default` | what an optional field falls back to when left blank; shown as a `Default: …` placeholder rather than prefilled, so the box reads as safe to skip |
| `hint` | placeholder text; overrides the `Default: …` placeholder |
| `required` | default `true`; an optional field left blank with no default simply isn't stored, and the backend applies its own |
| `options_query` | GraphQL query, run with the user's own credentials, whose results suggest values for this field |
| `sync_mutation` | GraphQL mutation run after a save, telling the backend what was picked. Takes the value as `$value` |

`options_query` exists because some values only the backend knows — which
calendars an account has, say. Alias the selection to `options { value label
disabled isDefault }` and the dashboard needs no per-field mapping config; the
query defines the shape. A Relay connection is unwrapped, so `options { nodes
{ … } }` works too — which is what `caldav-cli` returns, and why the shipped
query passes `first: 100` (collections there default to 25). Key `value` to an
id rather than a display name where the backend has one: two calendars can
share a name, and picking between them by name is a coin flip. An option is
offered unless `disabled` is true or it carries `supportsEvents: false`. Such a field
appears only once the account is connected (there is nobody to ask before
that), and is rejected at boot if marked `required`, since the connect form
can't show it. It renders as a `<datalist>`, so a backend that's down costs
suggestions, not the ability to type a name.

`sync_mutation` keeps the backend's own idea of a setting in step with ours —
picking a calendar here also points the account's default at it
(`setDefaultCalendar`), so the user's phone agrees with the gateway. It runs
only when the value changed, and the value is passed as a variable rather than
interpolated, since a calendar named `"` would otherwise rewrite the mutation.

**Our stored value is authoritative.** It is what the proxy injects on every
request, so the gateway behaves as asked whether or not the backend accepted
the sync. A refusal — no scheduling support, a read-only account — leaves the
save standing and is reported in the flash message. Mutations used this way
have to expose `success` and `error`, because a backend declining a valid
write answers with a payload saying so rather than a GraphQL error.

The query goes to the MCP's `graphql` key when it has one — a plain
GraphQL-over-HTTP endpoint the backend serves alongside `/mcp`
(`caldav-cli mcp --http … --graphql`), reached service-to-service and never
proxied for clients. Without it the same query goes through the `graphql` MCP
tool instead, which costs a session handshake and arrives wrapped in JSON-RPC,
wrapped in a tool result, as a string. MCP is a tool-call protocol for models;
between two services it is all envelope.

Editing a connected MCP leaves secrets blank to keep the stored value —
otherwise changing one field would mean retyping the app password. A field the
form never carried keeps its stored value too, which is what stops a credential
save from wiping a setting that saves itself elsewhere; only a visibly-cleared
non-secret is treated as cleared.

Set `credential_header` **or** `fields`, never both. The dashboard builds its
form from whichever is present, and the proxy strips every declared header from
the incoming request before injecting the real values — a client can't smuggle
its own. Header names are validated at boot, so a malformed one fails fast
rather than deep inside a request.

Storage did not change shape: a credential set is JSON-encoded into the same
single encrypted column. Rows written before this (a bare secret rather than a
JSON object) decode onto the MCP's first field, so **existing Fastmail tokens
keep working with no migration**.

## Deploy

`docker-compose.yml` is the **source of truth for the topology**; production is
that same shape translated to whatever runs it. This repo holds no cluster
manifests by design. See [`DEPLOY.md`](DEPLOY.md) for the prod deltas (domains,
TLS, admin-not-exposed, secrets from the backend, opaque tokens).

### Releases

Images are published to `ghcr.io/radiosilence/mcp-gateway`, tagged `vX.Y.Z`,
`vX.Y`, `vX` and `latest` for a release, and `main` / `sha-<short>` for every
commit on main. Pin whichever you want to follow — a deployment wanting to know
what changed should be on an exact version and read
[`CHANGELOG.md`](CHANGELOG.md).

Releasing is a version bump: change `version` in `Cargo.toml`, add the matching
`CHANGELOG.md` entry, and merging cuts the tag, the GitHub release and the
semver image tags. There is no tag to push by hand.

## Security model

Built to be internet-facing. The load-bearing pieces:

- **The login allowlist is not here.** DCR is public because Claude requires it,
  so what stops a stranger who registered a client from getting a token is that
  tokens issue only to people the login provider admits. That check lives there,
  once, for every service behind it — a second copy here would be a second thing
  to keep in step and a second way to lock somebody out. Reaching this gateway
  with a valid token *is* the check having passed.
- **Opaque tokens** — access tokens are introspected at Hydra per request; no
  JWT reaches a client, and tokens are revocable.
- **Per-IP rate limiting** on the auth routes (forwarded-IP aware, since a
  reverse proxy fronts this).
- **Credentials encrypted at rest** — per-user MCP keys are sealed with
  XChaCha20-Poly1305. `TOKEN_ENC_KEY` is the only thing protecting them: keep it
  in a real secret store, never in the image or git. Rotating it orphans stored
  keys (users re-paste) — acceptable given disposable state, but deliberate.
- **Hydra admin API is never exposed** — introspection uses the
  cluster-internal admin port only. It answers for any token without
  authenticating the caller.
- **Custodial by nature** — the gateway holds credentials for every user and MCP
  (full-mailbox Fastmail tokens; iCloud app passwords, which grant calendar and
  contacts access to the Apple ID). Tight RBAC on the secret and the DB is on
  you.
