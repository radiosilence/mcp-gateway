# Deploying

**`docker-compose.yml` is the source of truth for the topology.** It describes
every component, their images, env, ports, and dependencies. Production is that
same topology translated to jaritanet's Pulumi — this repo intentionally holds
*no* cluster manifests (no k8s YAML, no Terraform). Keep deployment specifics in
jaritanet; keep the shape here.

## Components (from `docker-compose.yml`)

| Component | Image | Notes |
|---|---|---|
| `postgres` | `postgres:16` | one instance, two DBs: `fastmail_mcp` (ours) + `hydra` |
| `hydra-migrate` | `oryd/hydra:v2.2.0` | one-shot `migrate sql` before Hydra serves |
| `hydra` | `oryd/hydra:v2.2.0` | the OAuth AS; public :4444, admin :4445 |
| `service` | built from `Dockerfile` | our Rust app; :8080 |

## Prod deltas (what Pulumi changes vs compose)

Everything else stays as in compose. The deltas:

- **Domains / URLs**: `PUBLIC_URL=https://fastmail.radiosilence.dev`,
  `HYDRA_ISSUER=https://auth.radiosilence.dev`, and Hydra's
  `URLS_SELF_ISSUER` / `URLS_LOGIN` / `URLS_CONSENT` to the real hosts.
  `HYDRA_ADMIN_URL` stays cluster-internal (e.g. `http://hydra-admin:4445`).
- **TLS + routing**: ingress terminates TLS (cert-manager / LE) and routes
  `fastmail.radiosilence.dev` → service:8080, `auth.radiosilence.dev` →
  hydra:4444. Two hosts, two backends; the service never fronts Hydra.
- **Hydra admin (:4445) is NOT exposed** — ClusterIP / internal only. Reachable
  from the service pods, never from the ingress.
- **Drop `--dev`**: run Hydra `serve all` (no `--dev`). Dev mode relaxes
  security and permits http/lax DCR — unsafe for prod. Configure real DCR policy
  and `SERVE_PUBLIC_CORS_ENABLED` as needed.
- **Secrets from the secret backend** (SOPS/Vault/sealed-secrets), never inline:
  `POSTGRES_PASSWORD`, `DATABASE_URL`, Hydra `DSN`, `SECRETS_SYSTEM`,
  `TOKEN_ENC_KEY` (32-byte base64), `GITHUB_CLIENT_ID/SECRET`. The
  `TOKEN_ENC_KEY` is the one that unlocks every stored Fastmail token — tightest
  RBAC, never in an image or in git.
- **Access tokens stay opaque** (Hydra default; we introspect). Do not set
  `STRATEGIES_ACCESS_TOKEN=jwt`.
- **Scale**: the `service` is stateless — run multiple replicas. Postgres single
  instance, small volume (state is disposable: lose it → users re-paste tokens).

## GitHub OAuth app

One app per environment (GitHub allows one callback URL each):
callback = `${PUBLIC_URL}/auth/github/callback`. Device Flow: off.
