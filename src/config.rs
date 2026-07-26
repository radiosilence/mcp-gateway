//! Runtime configuration, entirely from environment variables.
//!
//! Nothing here is secret-bearing beyond what the process needs to boot; the
//! master encryption key and OAuth client secret arrive via env (k8s Secret in
//! production) and are never logged.

use anyhow::{Context, Result};
use serde::Deserialize;

/// One value a backend MCP needs from the user.
///
/// Most MCPs want a single bearer token, but not all: CalDAV authenticates
/// with a username *and* an app password, against a server URL that varies by
/// provider. So a credential is a set of named fields, each injected into its
/// own header.
#[derive(Clone, Debug, Deserialize)]
pub struct CredentialField {
    /// Form field name and storage key, e.g. `"username"`.
    pub id: String,
    /// Label shown in the dashboard.
    pub label: String,
    /// Header the backend expects this value in, e.g. `X-CalDAV-Username`.
    pub header: String,
    /// Render as a password input and never echo the stored value back.
    /// Non-secret fields (a server URL) are shown so they can be edited.
    #[serde(default = "default_true")]
    pub secret: bool,
    /// Prefilled value — for a field with a sensible provider default.
    #[serde(default)]
    pub default: Option<String>,
    /// Placeholder text for the input.
    #[serde(default)]
    pub hint: Option<String>,
    /// Whether the user must fill this in. An optional field left blank is
    /// simply not stored, and the backend applies its own default.
    #[serde(default = "default_true")]
    pub required: bool,
    /// GraphQL query whose results become this field's suggestions, run against
    /// the backend's `graphql` tool with the user's own credentials. Alias the
    /// selection to `options { value label }` — the dashboard reads that shape
    /// and nothing else, so no per-field mapping config is needed.
    ///
    /// Only offered once credentials are stored: without them there is nobody
    /// to ask.
    #[serde(default)]
    pub options_query: Option<String>,
    /// GraphQL mutation run after this field is saved, to tell the backend what
    /// the user picked — `setDefaultCalendar`, say, so their phone agrees with
    /// the gateway. Takes the new value as `$value`.
    ///
    /// Best-effort by definition: our stored value is what the proxy injects
    /// and is authoritative, so a backend that refuses (no scheduling support,
    /// a read-only account) leaves the save standing and says so.
    #[serde(default)]
    pub sync_mutation: Option<String>,
}

fn default_true() -> bool {
    true
}

/// A backend MCP the gateway fronts. The registry comes from the environment
/// (`MCP_REGISTRY`), so adding an MCP is config, not code.
#[derive(Clone, Debug, Deserialize)]
pub struct Mcp {
    /// URL slug + storage key, e.g. "fastmail" → routed at `/mcp/fastmail`.
    pub id: String,
    /// Human name for the dashboard.
    pub name: String,
    /// Backend MCP endpoint to proxy to, e.g. `http://fastmail-mcp:8080/mcp`.
    pub backend: String,
    /// Shorthand for the single-secret case: one header, no `fields` block.
    /// Normalised into a one-entry `fields` at load, so nothing downstream has
    /// to know which form the registry used.
    #[serde(default)]
    credential_header: Option<String>,
    /// The values this MCP needs. Non-empty after loading unless [`Mcp::public`]
    /// is set, which is the one case where an MCP legitimately needs nothing.
    #[serde(default)]
    pub fields: Vec<CredentialField>,
    /// This MCP reads something public and takes no credentials at all.
    ///
    /// Opt-in rather than inferred from an empty `fields`, because "no
    /// credentials declared" is far more often a typo than a decision, and
    /// silently fronting a backend with no key is the wrong way to find out.
    /// A public MCP shows no connect form: the OAuth login already established
    /// who the user is, and there is nothing further to store.
    #[serde(default)]
    pub public: bool,
    /// Plain GraphQL-over-HTTP endpoint, when the backend serves one (e.g.
    /// `caldav-cli mcp --http … --graphql`). The dashboard prefers it for its
    /// own lookups: MCP is a tool-call protocol for models, and machine-to-
    /// machine it costs a session handshake and two layers of unwrapping to
    /// send the same query. Absent ⇒ fall back to the `graphql` MCP tool.
    ///
    /// Never proxied — clients reach `/{id}` only, which goes to `backend`.
    #[serde(default)]
    pub graphql: Option<String>,
    /// Optional link shown in the dashboard for where to get the key.
    #[serde(default)]
    pub key_help_url: Option<String>,
    /// Optional hint text, used as the placeholder in the single-secret form.
    #[serde(default)]
    pub key_hint: Option<String>,
    /// How to ask this backend whether the stored credentials are any good.
    /// Absent means the gateway makes no claim either way, which is the honest
    /// answer for a backend that authenticates nothing.
    #[serde(default)]
    pub verify: Option<Verify>,
}

/// A query proving the stored credentials are accepted, and how to read it.
///
/// Backends disagree about how to report bad auth — some raise, some answer
/// calmly with a status — so this describes both shapes rather than forcing one.
/// What it will not do is guess: an error is never taken as proof a credential
/// is wrong, only an explicit [`Verify::rejected`] match is. A backend that is
/// merely unreachable must not have its user told to go and change a password.
#[derive(Clone, Debug, Deserialize)]
pub struct Verify {
    /// Run with the user's own credentials, like any other backend call.
    pub query: String,
    /// Dotted path to the value that answers the question. Absent means the
    /// query completing without error is itself the answer, which is all a
    /// backend that raises on bad auth can tell us.
    #[serde(default)]
    pub path: Option<String>,
    /// The value at `path` meaning the credentials work. Absent means any
    /// truthy value does.
    #[serde(default)]
    pub ok: Option<String>,
    /// The value at `path` meaning the backend actively rejected them. Absent
    /// means nothing is ever reported as rejected — only verified or unknown.
    #[serde(default)]
    pub rejected: Option<String>,
}

impl Mcp {
    /// Whether this MCP needs nothing from the user. Ready to connect the
    /// moment they log in.
    pub fn is_public(&self) -> bool {
        self.public
    }

    /// The storage key of the first field — where a legacy single-secret row
    /// (a bare string rather than a JSON object) is mapped on read.
    pub fn primary_field(&self) -> &str {
        self.fields
            .first()
            .map(|f| f.id.as_str())
            .unwrap_or("token")
    }
}

#[derive(Clone)]
pub struct Config {
    /// Address to bind the HTTP server to, e.g. `0.0.0.0:8080`.
    pub bind_addr: String,
    /// This service's public base URL, e.g. `https://mail.radiosilence.dev`.
    /// Used to build OAuth protected-resource metadata and redirect URIs.
    pub public_url: String,
    /// Postgres connection string.
    pub database_url: String,
    /// 32-byte key for XChaCha20-Poly1305, base64 (standard) encoded.
    pub token_enc_key: Vec<u8>,
    /// Hydra's public issuer URL — what Claude uses to reach the AS, advertised
    /// in the protected-resource metadata, e.g. `https://auth.radiosilence.dev`.
    pub hydra_issuer: String,
    /// Hydra admin API base, cluster-internal only, e.g. `http://hydra-admin:4445`.
    /// Used for token introspection and the login/consent handshake.
    pub hydra_admin_url: String,
    /// GitHub OAuth app credentials (upstream identity).
    pub github_client_id: String,
    pub github_client_secret: String,
    /// Allowlist of GitHub logins (lowercased) permitted to authenticate. Empty
    /// = allow anyone (dev only). On a public deployment this MUST be set, or
    /// anyone with a GitHub account can log in and store their own credentials.
    pub github_allowlist: Vec<String>,
    /// Registry of backend MCPs this gateway fronts.
    pub mcps: Vec<Mcp>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        let token_enc_key = {
            use base64::Engine;
            let raw = env("TOKEN_ENC_KEY")?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(raw.trim())
                .context("TOKEN_ENC_KEY must be base64")?;
            anyhow::ensure!(
                bytes.len() == 32,
                "TOKEN_ENC_KEY must decode to exactly 32 bytes (got {})",
                bytes.len()
            );
            bytes
        };

        Ok(Self {
            bind_addr: env_or("BIND_ADDR", "0.0.0.0:8080"),
            public_url: env("PUBLIC_URL")?,
            database_url: env("DATABASE_URL")?,
            token_enc_key,
            hydra_issuer: env("HYDRA_ISSUER")?,
            hydra_admin_url: env("HYDRA_ADMIN_URL")?,
            github_client_id: env("GH_CLIENT_ID")?,
            github_client_secret: env("GH_CLIENT_SECRET")?,
            github_allowlist: env_or("GH_ALLOWED", "")
                .split(',')
                .map(|s| s.trim().to_lowercase())
                .filter(|s| !s.is_empty())
                .collect(),
            mcps: load_registry(&env("MCP_REGISTRY")?)?,
        })
    }

    /// Whether a GitHub login may authenticate. Empty allowlist ⇒ anyone (dev).
    pub fn github_login_allowed(&self, login: &str) -> bool {
        self.github_allowlist.is_empty() || self.github_allowlist.contains(&login.to_lowercase())
    }

    /// The GitHub OAuth callback URL registered with the GitHub app.
    pub fn github_redirect_uri(&self) -> String {
        format!(
            "{}/auth/github/callback",
            self.public_url.trim_end_matches('/')
        )
    }

    pub fn mcp(&self, id: &str) -> Option<&Mcp> {
        self.mcps.iter().find(|m| m.id == id)
    }
}

/// Path segments the gateway serves itself — an MCP id here would shadow a
/// gateway route (MCPs are mounted at `/{id}`).
const RESERVED_IDS: &[&str] = &[
    "register",
    "login",
    "logout",
    "dashboard",
    "healthz",
    "auth",
    "assets",
    ".well-known",
];

/// Parse the registry document. YAML, and therefore JSON too — every JSON
/// document is valid YAML, so both forms load through one parser.
///
/// It arrives whole in `MCP_REGISTRY` rather than as a path to a file, because
/// the registry belongs to the deployment and not to this repo: whatever runs
/// the gateway already holds it as config and can hand it straight over. A file
/// would be a second copy to keep in sync, and a mounted ConfigMap changing
/// under a running pod is invisible to it — env is part of the pod spec, so
/// editing the registry rolls the deployment.
fn load_registry(raw: &str) -> Result<Vec<Mcp>> {
    let mcps: Vec<Mcp> = serde_norway::from_str(raw).context(
        "parsing MCP_REGISTRY, which holds the registry document itself \
         (YAML or JSON), not a path to one",
    )?;
    mcps.into_iter().map(normalize).collect()
}

/// Validate one registry entry and expand the `credential_header` shorthand
/// into the general `fields` form.
fn normalize(mut m: Mcp) -> Result<Mcp> {
    anyhow::ensure!(
        !m.id.is_empty() && !m.id.contains('/'),
        "invalid MCP id {:?}: must be a non-empty single path segment",
        m.id
    );
    anyhow::ensure!(
        !RESERVED_IDS.contains(&m.id.as_str()),
        "MCP id {:?} is reserved (shadows a gateway route)",
        m.id
    );

    if m.public {
        anyhow::ensure!(
            m.credential_header.is_none() && m.fields.is_empty(),
            "MCP {:?} is marked public but declares credentials — use one or the other",
            m.id
        );
        return Ok(m);
    }

    if m.fields.is_empty() {
        let header = m.credential_header.clone().with_context(|| {
            format!(
                "MCP {:?} defines neither `credential_header` nor `fields` \
                 (add \"public\": true if it genuinely needs none)",
                m.id
            )
        })?;
        m.fields = vec![CredentialField {
            id: "token".into(),
            label: "API key".into(),
            header,
            secret: true,
            default: None,
            hint: m.key_hint.clone(),
            required: true,
            options_query: None,
            sync_mutation: None,
        }];
    } else {
        anyhow::ensure!(
            m.credential_header.is_none(),
            "MCP {:?} sets both `credential_header` and `fields` — use one or the other",
            m.id
        );
    }

    let mut seen = std::collections::HashSet::new();
    for f in &m.fields {
        anyhow::ensure!(
            !f.id.is_empty() && seen.insert(f.id.as_str()),
            "MCP {:?} has an empty or duplicated field id {:?}",
            m.id,
            f.id
        );
        // A field the user can't see on the connect form can't be required:
        // its choices come from a backend that needs credentials first.
        anyhow::ensure!(
            f.options_query.is_none() || !f.required,
            "MCP {:?} field {:?} is required but its options come from the backend",
            m.id,
            f.id
        );
        // A header name with a space or newline in it would be rejected at
        // request time, deep inside the proxy. Catch it at boot instead.
        anyhow::ensure!(
            !f.header.is_empty() && f.header.bytes().all(|b| b.is_ascii_graphic() && b != b':'),
            "MCP {:?} field {:?} has an invalid header name {:?}",
            m.id,
            f.id,
            f.header
        );
    }
    Ok(m)
}

fn env(key: &str) -> Result<String> {
    std::env::var(key).with_context(|| format!("missing required env var {key}"))
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(doc: &str) -> Result<Vec<Mcp>> {
        load_registry(doc)
    }

    #[test]
    fn single_secret_shorthand_expands_to_one_field() {
        let mcps = parse(
            r#"[{"id":"fastmail","name":"Fastmail","backend":"http://x/mcp",
                 "credential_header":"X-Fastmail-Token","key_hint":"fmu1-…"}]"#,
        )
        .unwrap();
        let fields = &mcps[0].fields;
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].id, "token");
        assert_eq!(fields[0].header, "X-Fastmail-Token");
        assert!(fields[0].secret);
        assert!(fields[0].required);
        // The legacy key_hint becomes the field's placeholder.
        assert_eq!(fields[0].hint.as_deref(), Some("fmu1-…"));
        assert_eq!(mcps[0].primary_field(), "token");
    }

    #[test]
    fn multi_field_registry_entries_load_as_written() {
        let mcps = parse(
            r#"[{"id":"caldav","name":"Calendar","backend":"http://x/mcp","fields":[
                 {"id":"username","label":"Apple ID","header":"X-CalDAV-Username","secret":false},
                 {"id":"password","label":"App password","header":"X-CalDAV-Password"},
                 {"id":"url","label":"Server","header":"X-CalDAV-Url","secret":false,
                  "default":"https://caldav.icloud.com","required":false}]}]"#,
        )
        .unwrap();
        let fields = &mcps[0].fields;
        assert_eq!(fields.len(), 3);
        assert!(!fields[0].secret);
        // `secret` defaults to true when omitted.
        assert!(fields[1].secret);
        assert_eq!(
            fields[2].default.as_deref(),
            Some("https://caldav.icloud.com")
        );
        assert!(!fields[2].required);
        assert_eq!(mcps[0].primary_field(), "username");
    }

    #[test]
    fn a_public_mcp_needs_no_credentials() {
        let mcps =
            parse(r#"[{"id":"folk","name":"Folk","backend":"http://folk/mcp","public":true}]"#)
                .unwrap();
        assert!(mcps[0].is_public());
        assert!(mcps[0].fields.is_empty(), "a public MCP declares no fields");
    }

    #[test]
    fn a_public_mcp_may_not_also_declare_credentials() {
        // Both forms together means one of them is a mistake, and guessing
        // which would either leak a form onto a credential-less MCP or drop a
        // credential silently.
        assert!(
            parse(
                r#"[{"id":"x","name":"X","backend":"http://x/mcp","public":true,
                     "credential_header":"X-Token"}]"#
            )
            .is_err()
        );
    }

    #[test]
    fn an_mcp_is_not_public_by_default() {
        // The absence of credentials stays an error: it is far more often a
        // typo than a decision.
        let mcps = parse(
            r#"[{"id":"x","name":"X","backend":"http://x/mcp","credential_header":"X-Token"}]"#,
        )
        .unwrap();
        assert!(!mcps[0].is_public());
    }

    #[test]
    fn rejects_an_mcp_with_no_credential_definition() {
        let err = parse(r#"[{"id":"x","name":"X","backend":"http://x/mcp"}]"#).unwrap_err();
        assert!(err.to_string().contains("neither"));
    }

    #[test]
    fn rejects_mixing_both_credential_forms() {
        let err = parse(
            r#"[{"id":"x","name":"X","backend":"http://x/mcp","credential_header":"X-A",
                 "fields":[{"id":"a","label":"A","header":"X-A"}]}]"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("one or the other"));
    }

    #[test]
    fn rejects_duplicate_field_ids() {
        let err = parse(
            r#"[{"id":"x","name":"X","backend":"http://x/mcp","fields":[
                 {"id":"a","label":"A","header":"X-A"},
                 {"id":"a","label":"B","header":"X-B"}]}]"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("duplicated field id"));
    }

    #[test]
    fn rejects_header_names_that_could_not_be_sent() {
        for header in ["X Bad", "X-Bad\nInjected", "", "X-Bad:"] {
            let json = format!(
                r#"[{{"id":"x","name":"X","backend":"http://x/mcp","fields":[
                     {{"id":"a","label":"A","header":{}}}]}}]"#,
                serde_json::to_string(header).unwrap()
            );
            assert!(parse(&json).is_err(), "accepted bad header {header:?}");
        }
    }

    /// The registry is written as YAML by whoever deploys the gateway, so the
    /// parser has to accept it in that form and not only as JSON.
    #[test]
    fn a_yaml_registry_loads() {
        let mcps = parse(
            r#"
- id: caldav
  name: Calendar
  backend: http://x/mcp
  fields:
    - id: username
      label: Apple ID
      header: X-CalDAV-Username
      secret: false
    - id: password
      label: App password
      header: X-CalDAV-Password
"#,
        )
        .unwrap();
        assert_eq!(mcps[0].id, "caldav");
        assert_eq!(mcps[0].fields.len(), 2);
        assert!(!mcps[0].fields[0].secret);
        assert!(mcps[0].fields[1].secret);
    }

    /// Guards the dev registry embedded in `docker-compose.yml` — a typo there
    /// breaks the local stack, and it is the worked example the README points
    /// at, so it is also the documentation.
    #[test]
    fn the_dev_registry_loads() {
        let compose: serde_norway::Value =
            serde_norway::from_str(&std::fs::read_to_string("docker-compose.yml").unwrap())
                .expect("docker-compose.yml must parse");
        let raw = compose["services"]["gateway"]["environment"]["MCP_REGISTRY"]
            .as_str()
            .expect("the gateway service must set MCP_REGISTRY");
        // Compose escapes a literal `$` as `$$`; undo it so this checks the
        // document the container is actually handed.
        let mcps = load_registry(&raw.replace("$$", "$")).expect("the dev registry must load");

        let ids: Vec<&str> = mcps.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"fastmail"), "got {ids:?}");
        assert!(ids.contains(&"caldav"), "got {ids:?}");

        let caldav = mcps.iter().find(|m| m.id == "caldav").unwrap();
        let headers: Vec<&str> = caldav.fields.iter().map(|f| f.header.as_str()).collect();
        assert_eq!(
            headers,
            [
                "X-CalDAV-Username",
                "X-CalDAV-Password",
                "X-CalDAV-Url",
                "X-CalDAV-Calendar"
            ]
        );
        // Only the password is a secret; the username and server URL are
        // shown in the dashboard so they can be edited in place.
        assert_eq!(
            caldav.fields.iter().filter(|f| f.secret).count(),
            1,
            "exactly one CalDAV field should be a secret"
        );
        // The server URL is optional and defaults to iCloud.
        let url = caldav.fields.iter().find(|f| f.id == "url").unwrap();
        assert!(!url.required);
        assert_eq!(url.default.as_deref(), Some("https://caldav.icloud.com"));

        // The calendar list comes from the backend, so the field is optional
        // and hidden until the account is connected.
        let calendar = caldav.fields.iter().find(|f| f.id == "calendar").unwrap();
        assert!(!calendar.required);
        let query = calendar.options_query.as_deref().unwrap();
        assert!(
            query.contains("options:"),
            "aliased to the shape the form reads: {query}"
        );
        assert!(
            query.contains("value:") && query.contains("label:"),
            "got {query}"
        );
    }

    #[test]
    fn a_verify_block_loads_with_only_a_query() {
        // All a backend that raises on bad auth can support.
        let mcps = parse(
            r#"[{"id":"x","name":"X","backend":"http://x/mcp","credential_header":"X-T",
                 "verify":{"query":"{ session { username } }"}}]"#,
        )
        .unwrap();
        let v = mcps[0].verify.as_ref().unwrap();
        assert_eq!(v.query, "{ session { username } }");
        assert!(v.path.is_none() && v.ok.is_none() && v.rejected.is_none());
    }

    #[test]
    fn a_verify_block_carries_the_values_it_was_given() {
        let mcps = parse(
            r#"[{"id":"x","name":"X","backend":"http://x/mcp","credential_header":"X-T",
                 "verify":{"query":"{ viewer { status } }","path":"viewer.status",
                           "ok":"CONNECTED","rejected":"INVALID_CREDENTIALS"}}]"#,
        )
        .unwrap();
        let v = mcps[0].verify.as_ref().unwrap();
        assert_eq!(v.path.as_deref(), Some("viewer.status"));
        assert_eq!(v.ok.as_deref(), Some("CONNECTED"));
        assert_eq!(v.rejected.as_deref(), Some("INVALID_CREDENTIALS"));
    }

    #[test]
    fn an_mcp_without_a_verify_block_claims_nothing() {
        let mcps =
            parse(r#"[{"id":"x","name":"X","backend":"http://x/mcp","credential_header":"X-T"}]"#)
                .unwrap();
        assert!(mcps[0].verify.is_none());
    }

    #[test]
    fn rejects_a_required_field_whose_options_need_credentials() {
        let err = parse(
            r#"[{"id":"x","name":"X","backend":"http://x/mcp","fields":[
                 {"id":"a","label":"A","header":"X-A"},
                 {"id":"b","label":"B","header":"X-B","options_query":"{ options { value } }"}]}]"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("options come from the backend"));
    }

    #[test]
    fn rejects_reserved_and_malformed_ids() {
        for id in ["dashboard", "healthz", "a/b", ""] {
            let json = format!(
                r#"[{{"id":{},"name":"X","backend":"http://x/mcp","credential_header":"X-A"}}]"#,
                serde_json::to_string(id).unwrap()
            );
            assert!(parse(&json).is_err(), "accepted bad id {id:?}");
        }
    }
}
