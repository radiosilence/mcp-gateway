-- The sole sensitive table: sub -> encrypted Fastmail token.
-- `sub` is the OAuth subject from Hydra (e.g. "github:12345").
-- `enc_token` is base64(nonce || XChaCha20-Poly1305 ciphertext); the plaintext
-- token never lands here.
CREATE TABLE IF NOT EXISTS fastmail_tokens (
    sub        TEXT        PRIMARY KEY,
    enc_token  TEXT        NOT NULL,
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
