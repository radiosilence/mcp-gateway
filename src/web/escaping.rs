//! Every page, whole and in place, every region a page loads, and every
//! action's answer, drawn with markup-shaped text in every field the person,
//! the registry or a backend controls: the login, an MCP's name, a field's
//! label and hint, a stored value, a setting's choices, a backend's error and
//! its refusal of a sync. Each answer must carry that text escaped exactly
//! once, and no markup of the templates' or the kit's own may come out
//! escaped: that is a fragment spliced in as text, which shows a page of raw
//! HTML.
//!
//! It runs against a real Postgres, named by `TEST_DATABASE_URL`, and a fake
//! backend it serves itself. CI provides the database; without one it is
//! skipped locally and refused in CI.

use std::sync::Arc;
use std::time::Duration;

use askama_datastar::escaping::{Guard, escaped, get, send};
use axum::Router;
use axum::body::Body;
use axum::extract::{Json, Request};
use axum::http::{StatusCode, header};
use axum::routing::post;
use serde_json::{Value, json};

use crate::auth::cookie::SESSION_COOKIE;
use crate::auth::hydra::HydraAdmin;
use crate::config::{Config, Mcp};
use crate::crypto::Cipher;
use crate::state::AppState;
use crate::store::{CredentialSet, Store};

/// Text shaped like markup, with every character escaping changes.
const HOSTILE: &str = r#"<em>hostile "q" & 'x'</em>"#;
const WAIT: Duration = Duration::from_secs(5);

/// A backend answering the dashboard's questions with `HOSTILE` wherever it
/// can: in a setting's choices, as a query's error, as a sync's refusal.
async fn backend() -> String {
    let app = Router::new().route(
        "/graphql",
        post(|Json(body): Json<Value>| async move {
            let query = body["query"].as_str().unwrap_or_default();
            Json(if query.contains("viewer") {
                json!({"data": {"viewer": {"status": "CONNECTED"}}})
            } else if query.contains("broken") {
                json!({"errors": [{"message": HOSTILE}]})
            } else if query.contains("mutation") {
                json!({"data": {"set": {"success": false, "error": HOSTILE}}})
            } else {
                json!({"data": {"options": [
                    {"value": HOSTILE, "label": HOSTILE, "isDefault": true},
                    {"value": "plain", "label": HOSTILE},
                ]}})
            })
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{address}/graphql")
}

/// An MCP with every kind of field, one that is not connected yet, and one
/// that takes no credentials, each named with markup.
fn registry(graphql: &str) -> Vec<Mcp> {
    serde_json::from_value(json!([
        {
            "id": "hostile", "name": HOSTILE, "backend": "http://127.0.0.1:9/mcp",
            "graphql": graphql,
            "verify": {"query": "{ viewer { status } }", "path": "viewer.status", "ok": "CONNECTED"},
            "fields": [
                {"id": "username", "label": HOSTILE, "header": "X-User", "secret": false, "hint": HOSTILE},
                {"id": "password", "label": HOSTILE, "header": "X-Pass"},
                {"id": "calendar", "label": HOSTILE, "header": "X-Cal", "secret": false,
                 "required": false, "options_query": "{ options { value label } }",
                 "sync_mutation": "mutation($value: String!) { set(value: $value) { success error } }"},
                {"id": "broken", "label": HOSTILE, "header": "X-Broken", "secret": false,
                 "required": false, "options_query": "{ broken: options { value } }"},
            ],
        },
        {
            "id": "fresh", "name": HOSTILE, "backend": "http://127.0.0.1:9/mcp",
            "key_help_url": "https://example.test/keys?for=<em>",
            "fields": [{"id": "token", "label": HOSTILE, "header": "X-Token", "hint": HOSTILE}],
        },
        {"id": "open", "name": HOSTILE, "backend": "http://127.0.0.1:9/mcp", "no_auth": true},
    ]))
    .unwrap()
}

async fn state(database_url: &str) -> AppState {
    let store = Store::connect(database_url, Cipher::new(&[7; 32]).unwrap())
        .await
        .unwrap();
    store.migrate().await.unwrap();
    let http = reqwest::Client::new();
    let config = Config {
        bind_addr: String::new(),
        public_url: "https://gateway.test".into(),
        database_url: database_url.into(),
        token_enc_key: Vec::new(),
        hydra_issuer: "https://issuer.test".into(),
        hydra_admin_url: "http://127.0.0.1:9".into(),
        oidc_client_id: String::new(),
        oidc_client_secret: String::new(),
        mcps: registry(&backend().await),
    };
    AppState {
        hydra: HydraAdmin::new(&config.hydra_admin_url, &config.public_url, http.clone()).unwrap(),
        config: Arc::new(config),
        store,
        proxy_http: http.clone(),
        http,
        questions: Default::default(),
    }
}

/// A request as the signed-in page makes it: a page load, or Datastar's
/// request when `in_place`, a form's fields as the body of a POST.
fn signed_in(
    method: &str,
    address: &str,
    session: &str,
    in_place: bool,
    form: Option<String>,
) -> Request {
    let mut request = Request::builder()
        .method(method)
        .uri(address)
        .header(header::COOKIE, format!("{SESSION_COOKIE}={session}"));
    if in_place {
        request = request.header("datastar-request", "true");
    }
    match form {
        Some(form) => request
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(Body::from(form)),
        None => request.body(Body::empty()),
    }
    .unwrap()
}

#[tokio::test]
async fn every_answer_escapes_what_others_control_exactly_once() {
    let Ok(database_url) = std::env::var("TEST_DATABASE_URL") else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI runs the escaping guard: set TEST_DATABASE_URL to a Postgres"
        );
        eprintln!("skipped: set TEST_DATABASE_URL to a Postgres to run the escaping guard");
        return;
    };
    let s = state(&database_url).await;
    // A subject of its own, so runs against one database do not meet.
    let sub = format!("guard-{}", std::process::id());
    let session = s.store.create_session(&sub, HOSTILE, 600).await.unwrap();
    let mcp = s.config.mcp("hostile").unwrap().clone();
    s.store
        .update_credentials(&sub, &mcp.id, mcp.primary_field(), |_| {
            Ok::<_, ()>(CredentialSet::from(
                ["username", "password", "calendar"].map(|f| (f.to_string(), HOSTILE.to_string())),
            ))
        })
        .await
        .unwrap()
        .unwrap();

    let router = super::routes().with_state(s);
    let guard = Guard::new(&[HOSTILE])
        .templates(concat!(env!("CARGO_MANIFEST_DIR"), "/templates"))
        .unwrap()
        // What the kit's components write.
        .elements([
            "a", "button", "details", "dialog", "div", "form", "input", "label", "option", "p",
            "section", "select", "small", "span", "summary", "svg",
        ]);
    let shown = escaped(HOSTILE);
    let mut found = Vec::new();
    let mut check = |what: &str, status: StatusCode, answer: &str, shows: bool| {
        if !status.is_success() {
            found.push(format!("{what}: {status}"));
        }
        found.extend(
            guard
                .problems(answer)
                .iter()
                .map(|p| format!("{what}: {p}")),
        );
        if shows && !answer.contains(&shown) {
            found.push(format!("{what}: the hostile text is missing"));
        }
    };

    for in_place in [false, true] {
        let (status, answer) = get(&router, "/", in_place, WAIT).await;
        check(&format!("/ in place {in_place}"), status, &answer, false);
        let request = signed_in("GET", "/dashboard", &session, in_place, None);
        let (status, answer) = send(&router, request, WAIT).await;
        check(
            &format!("/dashboard in place {in_place}"),
            status,
            &answer,
            true,
        );
    }
    for region in [
        "/dashboard/hostile/status",
        "/dashboard/hostile/settings/calendar",
        "/dashboard/hostile/settings/broken",
    ] {
        let request = signed_in("GET", region, &session, true, None);
        let (status, answer) = send(&router, request, WAIT).await;
        check(
            region,
            status,
            &answer,
            region != "/dashboard/hostile/status",
        );
    }
    // The dashboard again, now that the backend's answers stand and are drawn
    // by the page itself.
    let request = signed_in("GET", "/dashboard", &session, false, None);
    let (status, answer) = send(&router, request, WAIT).await;
    check("/dashboard answered", status, &answer, true);

    let hostile = url::form_urlencoded::byte_serialize(HOSTILE.as_bytes()).collect::<String>();
    for (action, body) in [
        (
            "/dashboard/hostile/credentials",
            format!("username={hostile}&password="),
        ),
        (
            "/dashboard/hostile/settings/calendar",
            "value=plain".to_string(),
        ),
        ("/dashboard/fresh/credentials", "token=".to_string()),
        ("/dashboard/hostile/disconnect", String::new()),
    ] {
        let request = signed_in("POST", action, &session, true, Some(body));
        let (status, answer) = send(&router, request, WAIT).await;
        check(action, status, &answer, true);
    }

    assert!(found.is_empty(), "{}", found.join("\n"));
}
