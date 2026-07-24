-- Server-side session state. Cookies hold only opaque random ids; all state
-- lives here, so nothing sensitive or forgeable rides on the client and
-- sessions can be revoked by deletion.

-- Dashboard login sessions.
CREATE TABLE IF NOT EXISTS sessions (
    id         TEXT        PRIMARY KEY,
    sub        TEXT        NOT NULL,
    login      TEXT        NOT NULL DEFAULT '',
    expires_at TIMESTAMPTZ NOT NULL
);

-- Transient OAuth flow state (CSRF + optional Hydra login_challenge) carried
-- across the GitHub redirect. One-shot: consumed (deleted) on callback.
CREATE TABLE IF NOT EXISTS oauth_flows (
    id              TEXT        PRIMARY KEY,
    csrf            TEXT        NOT NULL,
    login_challenge TEXT,
    expires_at      TIMESTAMPTZ NOT NULL
);
