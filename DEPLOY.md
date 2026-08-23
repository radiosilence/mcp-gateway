# Deploying

**`docker-compose.yml` is the source of truth for the topology.** It describes
every component, their images, env, ports, and dependencies.

For Kubernetes, `deploy/pulumi` translates that same topology —
`@radiosilence/mcp-gateway-pulumi`, published from this repo at the crate's own
version. What it does *not* carry is the part that is not this project's:
**hostnames, credentials and the list of MCPs belong to the deployment that
instantiates it** and are passed in. There is still no registry of its own and
no static YAML; a chart is the implementation of the topology, not an instance
of it.

See `deploy/pulumi/README.md`.

## Components (from `docker-compose.yml`)

| Component | Image | Notes |
|---|---|---|
| `postgres` | `postgres:16` | one instance, two DBs: `mcp_gateway` (gateway) + `hydra` |
| `hydra-migrate` | `oryd/hydra:v2.2.0` | one-shot `migrate sql` before Hydra serves |
| `hydra` | `oryd/hydra:v2.2.0` | the OAuth AS; public :4444, admin :4445 |
| `gateway` | built from `Dockerfile` | the gateway (lean Rust); :8080; registry via `MCP_REGISTRY` |
| `fastmail-mcp` | `ghcr.io/radiosilence/fastmail-cli` | backend MCP (`mcp --http`); internal :8080 |
| `caldav-mcp` | `ghcr.io/radiosilence/caldav-cli` | backend MCP (`mcp --http --graphql`); internal :8080 |
| `folk-mcp` | `ghcr.io/radiosilence/mainlynorfolk-mcp` | backend MCP, credential-less; internal :8080 |
| `tfl-mcp` | `ghcr.io/radiosilence/tfl-cli` | backend MCP; internal :8080 |

Backend images come from the CLI repos, which publish multi-arch builds on
release — the same images production pulls. This repo builds only the gateway.

Each additional MCP is another backend pod plus its registry entry. The registry
is passed whole in `MCP_REGISTRY`, so the two are declared together wherever the
deployment lives — and because env is part of the pod spec, editing the registry
rolls the gateway rather than leaving it on a stale copy.

## Prod deltas (what Pulumi changes vs compose)

Everything else stays as in compose. The deltas:

  Domains: **prod = `blit.cc`, dev = `radiosilence.dev`**.
- **Domains / URLs**: `PUBLIC_URL=https://mcp.blit.cc`,
  `HYDRA_ISSUER=https://auth.blit.cc`, and Hydra's
  `URLS_SELF_ISSUER` / `URLS_LOGIN` / `URLS_CONSENT` to the real hosts.
  `HYDRA_ADMIN_URL` stays cluster-internal (e.g. `http://hydra-admin:4445`).
- **TLS + routing**: ingress terminates TLS (cert-manager / LE) and routes
  `mcp.blit.cc` → gateway:8080, `auth.blit.cc` → hydra:4444. Two hosts, two
  backends; the gateway never fronts Hydra. Backend MCP pods (fastmail-mcp, …)
  are internal — only the gateway reaches them. Connector URL: `https://mcp.blit.cc/<id>`.
- **Hydra admin (:4445) is NOT exposed** — ClusterIP / internal only. Reachable
  from the gateway pods, never from the ingress.
- **Drop `--dev`**: run Hydra `serve all` (no `--dev`). Dev mode relaxes
  security and permits http/lax DCR — unsafe for prod. Configure real DCR policy
  and `SERVE_PUBLIC_CORS_ENABLED` as needed.
- **Secrets from the secret backend** (SOPS/Vault/sealed-secrets), never inline:
  `POSTGRES_PASSWORD`, `DATABASE_URL`, Hydra `DSN`, `SECRETS_SYSTEM`,
  `TOKEN_ENC_KEY` (32-byte base64), `OIDC_CLIENT_SECRET`. The
  `TOKEN_ENC_KEY` is the one that unlocks every stored credential — tightest
  RBAC, never in an image or in git.
- **Access tokens stay opaque** (Hydra default; we introspect). Do not set
  `STRATEGIES_ACCESS_TOKEN=jwt`.
- **Scale**: the `gateway` is stateless — run multiple replicas. Backend MCP
  pods scale independently. Postgres single instance, small volume (state is
  disposable: lose it → users re-paste keys).

## Registering with the login provider

This gateway is an ordinary relying party: it needs a client id, a secret and a
redirect URI of `${PUBLIC_URL}/auth/callback` at the issuer. In a deployment
that generates those (as jaritanet's does, deriving the redirect from the
hostname the service already publishes), there is nothing to do here by hand.

There is no GitHub OAuth app any more. Which upstream vouches for a person is
the login provider's business, and nothing here changes when that answer does.
