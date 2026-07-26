-- Postgres init: the app DB (mcp_gateway) is created by POSTGRES_DB; Hydra
-- needs its own, created here. Both live in one instance for local dev.
CREATE DATABASE hydra;
