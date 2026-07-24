-- Generalize the single-MCP token table into a per-(user, MCP) credential
-- vault. `sub` is the OAuth subject; `mcp_id` is the registry id (e.g.
-- "fastmail"); `enc_secret` is base64(nonce || XChaCha20-Poly1305 ciphertext).
CREATE TABLE IF NOT EXISTS credentials (
    sub        TEXT        NOT NULL,
    mcp_id     TEXT        NOT NULL,
    enc_secret TEXT        NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (sub, mcp_id)
);

DROP TABLE IF EXISTS fastmail_tokens;
