#!/usr/bin/env bash
# Provision a Cloudflare named tunnel for local Claude-connector testing.
#
# One tunnel, two public hostnames (this service + Hydra). Idempotent — safe to
# re-run. After this, `docker compose --profile tunnel up` runs everything
# including the tunnel (it mounts the cloudflared/ dir this writes).
#
# Usage:
#   FASTMAIL_HOST=fastmail-dev.radiosilence.dev \
#   AUTH_HOST=auth-dev.radiosilence.dev \
#   scripts/provision-tunnel.sh
set -euo pipefail

TUNNEL_NAME="${TUNNEL_NAME:-fastmail-mcp-dev}"
: "${FASTMAIL_HOST:?set FASTMAIL_HOST, e.g. fastmail-dev.radiosilence.dev}"
: "${AUTH_HOST:?set AUTH_HOST, e.g. auth-dev.radiosilence.dev}"

REPO_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${REPO_DIR}/cloudflared"
mkdir -p "${OUT_DIR}"

for bin in cloudflared jq; do
  command -v "$bin" >/dev/null || { echo "error: '$bin' not installed"; exit 1; }
done

# 1. Browser login only if there's no cert yet.
if [ ! -f "${HOME}/.cloudflared/cert.pem" ]; then
  echo "==> cloudflared not logged in — opening browser..."
  cloudflared tunnel login
fi

# 2. Create the tunnel if it doesn't already exist.
if ! cloudflared tunnel list --output json \
     | jq -e --arg n "$TUNNEL_NAME" '.[] | select(.name==$n)' >/dev/null; then
  echo "==> creating tunnel '${TUNNEL_NAME}'"
  cloudflared tunnel create "${TUNNEL_NAME}"
fi
UUID="$(cloudflared tunnel list --output json \
        | jq -r --arg n "$TUNNEL_NAME" '.[] | select(.name==$n) | .id')"
echo "==> tunnel id: ${UUID}"

# 3. Point both hostnames at the tunnel (tolerate re-runs).
cloudflared tunnel route dns "${TUNNEL_NAME}" "${FASTMAIL_HOST}" || true
cloudflared tunnel route dns "${TUNNEL_NAME}" "${AUTH_HOST}" || true

# 4. Stage credentials + config for the compose container to mount.
cp "${HOME}/.cloudflared/${UUID}.json" "${OUT_DIR}/creds.json"
cat > "${OUT_DIR}/config.yml" <<EOF
tunnel: ${UUID}
credentials-file: /etc/cloudflared/creds.json
ingress:
  - hostname: ${FASTMAIL_HOST}
    service: http://service:8080
  - hostname: ${AUTH_HOST}
    service: http://hydra:4444
  - service: http_status:404
EOF
echo "==> wrote cloudflared/config.yml + creds.json (gitignored)"

cat <<EOF

Next:
  1. In .env set:
       PUBLIC_URL=https://${FASTMAIL_HOST}
       HYDRA_ISSUER=https://${AUTH_HOST}
  2. GitHub OAuth app callback -> https://${FASTMAIL_HOST}/auth/github/callback
  3. docker compose --profile tunnel up
  4. Add https://${FASTMAIL_HOST}/mcp as a custom connector in Claude
EOF
