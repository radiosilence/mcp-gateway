//! Just enough MCP client to ask a backend a question on the user's behalf.
//!
//! The proxy is otherwise a dumb pipe — the client does the protocol. But the
//! dashboard needs to populate a field's choices (which calendars does this
//! account have?), and only the backend knows. So: initialise a session, call
//! the `graphql` tool with the user's own credentials, tear the session down.
//!
//! Streamable HTTP answers a POST with either JSON or a one-frame SSE stream,
//! depending on the server; both are handled.

use anyhow::{Context, Result, bail};
use axum::http::header;
use serde_json::{Value, json};

use crate::config::Mcp;
use crate::state::AppState;
use crate::store::CredentialSet;

const PROTOCOL_VERSION: &str = "2024-11-05";
const SESSION_HEADER: &str = "mcp-session-id";

/// Run `query` against `mcp`'s `graphql` tool as the owner of `credentials`,
/// returning the query's `data` object.
pub async fn graphql(
    state: &AppState,
    mcp: &Mcp,
    credentials: &CredentialSet,
    query: &str,
) -> Result<Value> {
    let session = initialize(state, mcp, credentials).await?;

    let result = call_tool(state, mcp, credentials, &session, query).await;

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

/// A request to the backend carrying the user's credential headers.
fn request(
    state: &AppState,
    mcp: &Mcp,
    credentials: &CredentialSet,
    session: Option<&str>,
) -> reqwest::RequestBuilder {
    let mut req = state
        .http
        .post(&mcp.backend)
        // Streamable HTTP lets the server pick; accept both.
        .header(header::ACCEPT, "application/json, text/event-stream")
        .header(header::CONTENT_TYPE, "application/json");
    for field in &mcp.fields {
        if let Some(value) = credentials.get(&field.id).filter(|v| !v.is_empty())
            && let Ok(v) = header::HeaderValue::from_str(value)
        {
            req = req.header(field.header.as_str(), v);
        }
    }
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
) -> Result<Value> {
    let response = request(state, mcp, credentials, session.as_deref())
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "graphql", "arguments": {"query": query}},
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
    if let Some(errors) = parsed.get("errors").and_then(Value::as_array)
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
    parsed
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
