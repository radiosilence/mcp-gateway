-- This gateway stopped being Hydra's login provider and became an ordinary
-- relying party, so the flow row carries a different secret across the redirect.
--
-- `login_challenge` was Hydra asking us to authenticate somebody; nothing sends
-- one any more. `verifier` is the PKCE verifier we hold back while the browser
-- is away, and redeem with the authorization code.
--
-- In-flight logins do not survive this, which costs a retry: the rows are
-- one-shot and expire in ten minutes anyway.
DELETE FROM oauth_flows;

ALTER TABLE oauth_flows DROP COLUMN IF EXISTS login_challenge;
ALTER TABLE oauth_flows ADD COLUMN IF NOT EXISTS verifier TEXT NOT NULL DEFAULT '';
