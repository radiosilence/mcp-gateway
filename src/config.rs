//! Runtime configuration, entirely from environment variables. The encryption
//! key and OIDC client secret are never logged.

use anyhow::{Context, Result};
use serde::Deserialize;

/// One value a backend MCP needs from the user, injected into its own header.
#[derive(Clone, Debug, Deserialize)]
pub struct CredentialField {
    /// Form field name and storage key, e.g. `"username"`.
    pub id: String,
    /// Label shown in the dashboard.
    pub label: String,
    /// Header the backend expects this value in, e.g. `X-CalDAV-Username`.
    pub header: String,
    /// Render as a password input and never echo the stored value back.
    #[serde(default = "default_true")]
    pub secret: bool,
    /// Stored when an optional field is left blank.
    #[serde(default)]
    pub default: Option<String>,
    /// Placeholder text for the input.
    #[serde(default)]
    pub hint: Option<String>,
    /// Whether the user must fill this in.
    #[serde(default = "default_true")]
    pub required: bool,
    /// GraphQL query whose results become this field's choices, run with the
    /// user's credentials once they are stored. Alias the selection to
    /// `options { value label }`.
    #[serde(default)]
    pub options_query: Option<String>,
    /// GraphQL mutation run after this field changes, taking it as `$value`,
    /// so the backend's own setting agrees. Best effort: the stored value is
    /// what the proxy injects either way.
    #[serde(default)]
    pub sync_mutation: Option<String>,
}

fn default_true() -> bool {
    true
}

/// A backend MCP the gateway fronts, from `MCP_REGISTRY`.
#[derive(Clone, Debug, Deserialize)]
pub struct Mcp {
    /// URL slug and storage key: "fastmail" is routed at `/fastmail`.
    pub id: String,
    /// Human name for the dashboard.
    pub name: String,
    /// Backend MCP endpoint to proxy to, e.g. `http://fastmail-mcp:8080/mcp`.
    pub backend: String,
    /// Shorthand for a single secret, expanded into `fields` at load.
    #[serde(default)]
    credential_header: Option<String>,
    /// The values this MCP needs; non-empty after loading unless `public`.
    #[serde(default)]
    pub fields: Vec<CredentialField>,
    /// Takes no credentials. Explicit rather than inferred from empty `fields`,
    /// which is more often a mistake than a decision.
    #[serde(default)]
    pub public: bool,
    /// Plain GraphQL-over-HTTP endpoint for the dashboard's own queries, which
    /// otherwise go through the `graphql` MCP tool. Never proxied.
    #[serde(default)]
    pub graphql: Option<String>,
    /// Optional link shown in the dashboard for where to get the key.
    #[serde(default)]
    pub key_help_url: Option<String>,
    /// Optional hint text, used as the placeholder in the single-secret form.
    #[serde(default)]
    pub key_hint: Option<String>,
    /// How to ask the backend whether the stored credentials work. Absent, the
    /// gateway makes no claim.
    #[serde(default)]
    pub verify: Option<Verify>,
}

/// A query proving the stored credentials are accepted, and how to read it.
/// Only an explicit [`Verify::rejected`] match reports a credential as wrong;
/// an error is never taken as proof.
#[derive(Clone, Debug, Deserialize)]
pub struct Verify {
    /// Run with the user's own credentials, like any other backend call.
    pub query: String,
    /// Dotted path to the answer. Absent, completing without error is the
    /// answer.
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
    /// The field a legacy single-secret row decodes onto.
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
    /// This service's public base URL.
    pub public_url: String,
    /// Postgres connection string.
    pub database_url: String,
    /// 32-byte key for XChaCha20-Poly1305, base64 (standard) encoded.
    pub token_enc_key: Vec<u8>,
    /// Hydra's public issuer URL: the authorization server MCP clients are
    /// pointed at, and where the dashboard login goes.
    pub hydra_issuer: String,
    /// Hydra's cluster-internal admin API, for token introspection.
    pub hydra_admin_url: String,
    /// The dashboard's own client registration at the issuer.
    pub oidc_client_id: String,
    pub oidc_client_secret: String,
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
            oidc_client_id: env("OIDC_CLIENT_ID")?,
            oidc_client_secret: env("OIDC_CLIENT_SECRET")?,
            mcps: load_registry(&env("MCP_REGISTRY")?)?,
        })
    }

    pub fn oidc_issuer(&self) -> &str {
        self.hydra_issuer.trim_end_matches('/')
    }

    /// Derived from `PUBLIC_URL`, as the login provider derives the copy it
    /// registers.
    pub fn oidc_redirect_uri(&self) -> String {
        format!("{}/auth/callback", self.public_url.trim_end_matches('/'))
    }

    pub fn mcp(&self, id: &str) -> Option<&Mcp> {
        self.mcps.iter().find(|m| m.id == id)
    }
}

/// Path segments the gateway serves itself, which an MCP id would shadow.
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

/// Parse the registry document: YAML, which includes JSON. It is passed whole
/// in the environment rather than as a file path so that editing it rolls the
/// deployment.
fn load_registry(raw: &str) -> Result<Vec<Mcp>> {
    let mcps: Vec<Mcp> = serde_norway::from_str(raw).context(
        "parsing MCP_REGISTRY, which holds the registry document itself \
         (YAML or JSON), not a path to one",
    )?;
    let mcps: Vec<Mcp> = mcps.into_iter().map(normalize).collect::<Result<_>>()?;
    let mut seen = std::collections::HashSet::new();
    for m in &mcps {
        anyhow::ensure!(
            seen.insert(m.id.as_str()),
            "MCP id {:?} is duplicated",
            m.id
        );
    }
    Ok(mcps)
}

/// Ids end up in URLs, element ids and Datastar expressions, so they are kept
/// to characters that need no escaping in any of them.
fn is_slug(id: &str) -> bool {
    !id.is_empty()
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Validate one registry entry and expand the `credential_header` shorthand
/// into the general `fields` form.
fn normalize(mut m: Mcp) -> Result<Mcp> {
    anyhow::ensure!(
        is_slug(&m.id),
        "invalid MCP id {:?}: use letters, digits, `-` and `_`",
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
            is_slug(&f.id) && seen.insert(f.id.as_str()),
            "MCP {:?} has an invalid or duplicated field id {:?}",
            m.id,
            f.id
        );
        // Its choices need stored credentials, so it is not on the connect form.
        anyhow::ensure!(
            f.options_query.is_none() || !f.required,
            "MCP {:?} field {:?} is required but its options come from the backend",
            m.id,
            f.id
        );
        anyhow::ensure!(
            http::HeaderName::from_bytes(f.header.as_bytes()).is_ok(),
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
        assert!(mcps[0].public);
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
        assert!(!mcps[0].public);
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
    fn rejects_duplicate_mcp_ids() {
        let err = parse(
            r#"[{"id":"x","name":"X","backend":"http://x/mcp","public":true},
                {"id":"x","name":"Y","backend":"http://y/mcp","public":true}]"#,
        )
        .unwrap_err();
        assert!(err.to_string().contains("duplicated"));
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
        for id in ["dashboard", "healthz", "a/b", "", "it's", "a b", "a.b"] {
            let json = format!(
                r#"[{{"id":{},"name":"X","backend":"http://x/mcp","credential_header":"X-A"}}]"#,
                serde_json::to_string(id).unwrap()
            );
            assert!(parse(&json).is_err(), "accepted bad id {id:?}");
        }
    }
}
