-- Long-lived bearer tokens for callers that cannot do the OAuth flow. Only a
-- hash is kept; the token itself is shown once, when it is created.
CREATE TABLE IF NOT EXISTS access_tokens (
    id           TEXT        PRIMARY KEY,
    sub          TEXT        NOT NULL,
    name         TEXT        NOT NULL,
    token_hash   TEXT        NOT NULL UNIQUE,
    mcp_ids      TEXT[],      -- NULL: every MCP
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_used_at TIMESTAMPTZ
);
CREATE INDEX IF NOT EXISTS access_tokens_sub ON access_tokens (sub);
