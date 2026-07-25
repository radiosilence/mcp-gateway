//! Asking a backend a question on the user's behalf.
//!
//! The proxy is otherwise a dumb pipe — the client speaks the protocol. But the
//! dashboard needs to populate a field's choices (which calendars does this
//! account have?), and only the backend knows.
//!
//! A backend serving plain GraphQL gets one POST. One that speaks only MCP gets
//! the long way round: initialise a session, call the `graphql` tool, tear the
//! session down — and its answer arrives as JSON-RPC (bare or as an SSE frame)
//! wrapping a tool result wrapping the GraphQL response as a string.

use anyhow::{Context, Result, bail};
use axum::http::header;
use serde_json::{Value, json};

use crate::config::Mcp;
use crate::state::AppState;
use crate::store::CredentialSet;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SESSION_HEADER: &str = "mcp-session-id";

/// Run `query` against `mcp` as the owner of `credentials`, returning the
/// query's `data` object.
///
/// Straight to the backend's GraphQL endpoint where it has one; otherwise
/// through the `graphql` MCP tool, which costs a session handshake and arrives
/// wrapped in JSON-RPC, wrapped in a tool result, as a string.
pub async fn graphql(
    state: &AppState,
    mcp: &Mcp,
    credentials: &CredentialSet,
    query: &str,
    variables: Option<Value>,
) -> Result<Value> {
    match &mcp.graphql {
        Some(endpoint) => post_graphql(state, mcp, credentials, endpoint, query, variables).await,
        None => graphql_over_mcp(state, mcp, credentials, query, variables).await,
    }
}

/// One POST, one JSON response.
async fn post_graphql(
    state: &AppState,
    mcp: &Mcp,
    credentials: &CredentialSet,
    endpoint: &str,
    query: &str,
    variables: Option<Value>,
) -> Result<Value> {
    let mut body = json!({"query": query});
    if let Some(variables) = variables {
        body["variables"] = variables;
    }
    let response = credentialed(state.http.post(endpoint), mcp, credentials)
        .json(&body)
        .send()
        .await
        .context("backend unreachable")?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("backend returned {status}");
    }
    let parsed: Value = serde_json::from_str(&body).context("backend returned unparseable JSON")?;
    graphql_data(parsed)
}

async fn graphql_over_mcp(
    state: &AppState,
    mcp: &Mcp,
    credentials: &CredentialSet,
    query: &str,
    variables: Option<Value>,
) -> Result<Value> {
    let session = initialize(state, mcp, credentials).await?;

    let result = call_tool(state, mcp, credentials, &session, query, variables).await;

    // The backend keeps session state until told otherwise; drop it even if the
    // call failed, so a dashboard refresh loop can't pile sessions up.
    if let Some(id) = &session {
        let _ = state
            .http
            .delete(&mcp.backend)
            .header(SESSION_HEADER, id)
            .send()
            .await;
    }
    result
}

/// Attach the user's credentials as the headers this MCP declares. The backend
/// trusts them unconditionally — it is only reachable inside the cluster, and
/// the gateway is the thing that authenticated the user.
fn credentialed(
    mut req: reqwest::RequestBuilder,
    mcp: &Mcp,
    credentials: &CredentialSet,
) -> reqwest::RequestBuilder {
    for field in &mcp.fields {
        if let Some(value) = credentials.get(&field.id).filter(|v| !v.is_empty())
            && let Ok(v) = header::HeaderValue::from_str(value)
        {
            req = req.header(field.header.as_str(), v);
        }
    }
    req.header(header::CONTENT_TYPE, "application/json")
}

/// An MCP request to the backend.
fn request(
    state: &AppState,
    mcp: &Mcp,
    credentials: &CredentialSet,
    session: Option<&str>,
) -> reqwest::RequestBuilder {
    // Streamable HTTP lets the server pick its response encoding; accept both.
    let mut req = credentialed(state.http.post(&mcp.backend), mcp, credentials)
        .header(header::ACCEPT, "application/json, text/event-stream");
    if let Some(id) = session {
        req = req.header(SESSION_HEADER, id);
    }
    req
}

/// Handshake, returning the session id the backend assigned (if any).
async fn initialize(
    state: &AppState,
    mcp: &Mcp,
    credentials: &CredentialSet,
) -> Result<Option<String>> {
    let response = request(state, mcp, credentials, None)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "jaritanet-mcp-gateway",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            },
        }))
        .send()
        .await
        .context("backend unreachable")?;

    let session = response
        .headers()
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("backend rejected initialize: {status}");
    }
    rpc_result(&body)?;

    // Servers may refuse requests until this notification arrives.
    let _ = request(state, mcp, credentials, session.as_deref())
        .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .send()
        .await;

    Ok(session)
}

async fn call_tool(
    state: &AppState,
    mcp: &Mcp,
    credentials: &CredentialSet,
    session: &Option<String>,
    query: &str,
    variables: Option<Value>,
) -> Result<Value> {
    let mut arguments = json!({"query": query});
    // The tool takes its variables JSON-encoded, unlike the plain endpoint.
    if let Some(variables) = variables {
        arguments["variables"] = Value::String(variables.to_string());
    }
    let response = request(state, mcp, credentials, session.as_deref())
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "graphql", "arguments": arguments},
        }))
        .send()
        .await
        .context("backend unreachable")?;

    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!("backend returned {status}");
    }

    let result = rpc_result(&body)?;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        bail!(
            "{}",
            tool_text(&result).unwrap_or_else(|| "tool call failed".into())
        );
    }

    // A tool answers in text; this one's text is the GraphQL response.
    let text = tool_text(&result).context("backend returned no text content")?;
    let parsed: Value = serde_json::from_str(&text).context("backend returned unparseable JSON")?;
    graphql_data(parsed)
}

/// The `data` of a GraphQL response, surfacing the first error if it failed.
fn graphql_data(response: Value) -> Result<Value> {
    if let Some(errors) = response.get("errors").and_then(Value::as_array)
        && !errors.is_empty()
    {
        bail!(
            "{}",
            errors[0]
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("query failed")
        );
    }
    response
        .get("data")
        .cloned()
        .context("query returned no data")
}

fn tool_text(result: &Value) -> Option<String> {
    result
        .get("content")?
        .as_array()?
        .iter()
        .find_map(|c| c.get("text").and_then(Value::as_str))
        .map(str::to_string)
}

/// The `result` of a JSON-RPC reply delivered as either bare JSON or an SSE
/// frame, erroring on a JSON-RPC `error` response.
fn rpc_result(body: &str) -> Result<Value> {
    let payload = match body.lines().find_map(|l| l.strip_prefix("data:")) {
        Some(data) => data.trim(),
        None => body.trim(),
    };
    let message: Value =
        serde_json::from_str(payload).context("backend sent malformed JSON-RPC")?;
    if let Some(error) = message.get("error") {
        bail!(
            "{}",
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("backend error")
        );
    }
    message.get("result").cloned().context("no result in reply")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_result_out_of_an_sse_frame() {
        let body =
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}\n\n";
        assert_eq!(rpc_result(body).unwrap(), json!({"ok": true}));
    }

    #[test]
    fn reads_a_result_out_of_plain_json() {
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        assert_eq!(rpc_result(body).unwrap(), json!({"ok": true}));
    }

    #[test]
    fn surfaces_the_json_rpc_error_message() {
        let body = r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"no session"}}"#;
        assert_eq!(rpc_result(body).unwrap_err().to_string(), "no session");
    }

    #[test]
    fn pulls_text_out_of_tool_content() {
        let result = json!({"content": [{"type": "text", "text": "hello"}]});
        assert_eq!(tool_text(&result).as_deref(), Some("hello"));
        assert_eq!(tool_text(&json!({"content": []})), None);
    }
}
