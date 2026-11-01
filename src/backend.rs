//! Asking a backend a GraphQL question on the user's behalf, for the
//! dashboard's own needs (a setting's choices, a credential check).
//!
//! A backend serving plain GraphQL gets one POST. One that speaks only MCP gets
//! a session: initialise, call the `graphql` tool, tear down. The answer then
//! arrives as JSON-RPC (bare or over SSE) wrapping a tool result wrapping the
//! GraphQL response as a string.

use anyhow::{Context, Result, bail};
use axum::http::header;
use futures_util::StreamExt;
use serde_json::{Value, json};

use crate::config::Mcp;
use crate::state::AppState;
use crate::store::CredentialSet;

const PROTOCOL_VERSION: &str = "2025-06-18";
const SESSION_HEADER: &str = "mcp-session-id";
const VERSION_HEADER: &str = "mcp-protocol-version";
/// Far beyond any answer the dashboard asks for; bounds what a misbehaving
/// backend can make it buffer.
const MAX_REPLY: usize = 4 << 20;

/// Run `query` against `mcp` as the owner of `credentials`, returning the
/// query's `data` object.
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
    if !status.is_success() {
        bail!("backend returned {status}");
    }
    let parsed: Value = serde_json::from_slice(&read_capped(response).await?)
        .context("backend returned unparseable JSON")?;
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

    let mut arguments = json!({"query": query});
    // The tool takes its variables JSON-encoded, unlike the plain endpoint.
    if let Some(variables) = variables {
        arguments["variables"] = Value::String(variables.to_string());
    }
    let response = session
        .request(state.http.post(&mcp.backend), mcp, credentials)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "graphql", "arguments": arguments},
        }))
        .send()
        .await
        .context("backend unreachable")?;
    let result = rpc_result(response, 2).await?;
    if result.get("isError").and_then(Value::as_bool) == Some(true) {
        bail!(
            "{}",
            tool_text(&result).unwrap_or_else(|| "tool call failed".into())
        );
    }
    let text = tool_text(&result).context("backend returned no text content")?;
    let parsed: Value = serde_json::from_str(&text).context("backend returned unparseable JSON")?;
    graphql_data(parsed)
}

/// A backend session, ended when this is dropped: that covers an error part
/// way through and the dashboard request being cancelled mid-await. The
/// teardown runs as its own task so nobody waits for it.
struct Session {
    id: Option<String>,
    version: String,
    /// Carries the same credentials as every other request, since a backend
    /// that authenticates each request refuses a bare DELETE. The protocol
    /// version is added at drop, once negotiated.
    teardown: Option<reqwest::RequestBuilder>,
}

impl Session {
    fn request(
        &self,
        req: reqwest::RequestBuilder,
        mcp: &Mcp,
        credentials: &CredentialSet,
    ) -> reqwest::RequestBuilder {
        let req = mcp_request(req, mcp, credentials).header(VERSION_HEADER, &self.version);
        match &self.id {
            Some(id) => req.header(SESSION_HEADER, id),
            None => req,
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let Some(request) = self.teardown.take() else {
            return;
        };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let request = request.header(VERSION_HEADER, &self.version);
        runtime.spawn(async move {
            let _ = request.send().await;
        });
    }
}

/// Attach the user's credentials as the headers this MCP declares.
fn credentialed(
    mut req: reqwest::RequestBuilder,
    mcp: &Mcp,
    credentials: &CredentialSet,
) -> reqwest::RequestBuilder {
    for field in &mcp.fields {
        if let Some(value) = credentials.get(&field.id).filter(|v| !v.is_empty())
            && let Ok(mut v) = header::HeaderValue::from_str(value)
        {
            v.set_sensitive(true);
            req = req.header(&field.header, v);
        }
    }
    req.header(header::CONTENT_TYPE, "application/json")
}

/// Streamable HTTP lets the server choose JSON or SSE, so accept both.
fn mcp_request(
    req: reqwest::RequestBuilder,
    mcp: &Mcp,
    credentials: &CredentialSet,
) -> reqwest::RequestBuilder {
    credentialed(req, mcp, credentials)
        .header(header::ACCEPT, "application/json, text/event-stream")
}

async fn initialize(state: &AppState, mcp: &Mcp, credentials: &CredentialSet) -> Result<Session> {
    let response = mcp_request(state.http.post(&mcp.backend), mcp, credentials)
        .json(&json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {"name": "mcp-gateway", "version": crate::VERSION},
            },
        }))
        .send()
        .await
        .context("backend unreachable")?;

    // Built before anything can fail, so every early return ends the session.
    let id = response
        .headers()
        .get(SESSION_HEADER)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let mut session = Session {
        teardown: id.as_ref().map(|id| {
            credentialed(state.http.delete(&mcp.backend), mcp, credentials)
                .header(SESSION_HEADER, id)
        }),
        id,
        version: PROTOCOL_VERSION.to_string(),
    };
    let result = rpc_result(response, 1).await?;
    if let Some(version) = result
        .get("protocolVersion")
        .and_then(Value::as_str)
        .filter(|v| header::HeaderValue::from_str(v).is_ok())
    {
        session.version = version.to_string();
    }

    // Servers may refuse requests until this notification arrives.
    let _ = session
        .request(state.http.post(&mcp.backend), mcp, credentials)
        .json(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
        .send()
        .await;

    Ok(session)
}

/// The `data` of a GraphQL response, surfacing the first error if it failed.
fn graphql_data(response: Value) -> Result<Value> {
    if let Some(error) = response
        .get("errors")
        .and_then(Value::as_array)
        .and_then(|e| e.first())
    {
        bail!(
            "{}",
            error
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

/// The `result` of the JSON-RPC reply to request `id`.
///
/// Over SSE the stream is read only until that reply arrives: a server may send
/// notifications first, and may hold the stream open afterwards.
async fn rpc_result(response: reqwest::Response, id: u64) -> Result<Value> {
    let status = response.status();
    if !status.is_success() {
        bail!("backend returned {status}");
    }
    let is_sse = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("text/event-stream"));

    let reply = match is_sse {
        false => serde_json::from_slice(&read_capped(response).await?)
            .context("backend sent malformed JSON-RPC")?,
        true => {
            let mut events = SseEvents::default();
            let mut body = response.bytes_stream();
            loop {
                if let Some(reply) = events.by_ref().find(|m| has_id(m, id)) {
                    break reply;
                }
                anyhow::ensure!(
                    events.buffer.len() <= MAX_REPLY,
                    "backend's reply is larger than {MAX_REPLY} bytes"
                );
                match body.next().await {
                    Some(chunk) => events.push(&chunk.context("reading the backend's stream")?),
                    None => {
                        events.finish();
                        break events
                            .find(|m| has_id(m, id))
                            .context("backend sent no JSON-RPC reply")?;
                    }
                }
            }
        }
    };
    reply_result(reply)
}

async fn read_capped(response: reqwest::Response) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk.context("reading the backend's reply")?);
        anyhow::ensure!(
            body.len() <= MAX_REPLY,
            "backend's reply is larger than {MAX_REPLY} bytes"
        );
    }
    Ok(body)
}

fn has_id(message: &Value, id: u64) -> bool {
    message.get("id").and_then(Value::as_u64) == Some(id)
}

fn reply_result(reply: Value) -> Result<Value> {
    if let Some(error) = reply.get("error") {
        bail!(
            "{}",
            error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("backend error")
        );
    }
    reply.get("result").cloned().context("no result in reply")
}

/// Incremental SSE parsing: bytes go in, the JSON payload of each complete
/// event comes out. Events whose data is not JSON are skipped.
#[derive(Default)]
struct SseEvents {
    buffer: Vec<u8>,
}

impl SseEvents {
    fn push(&mut self, chunk: &[u8]) {
        self.buffer.extend_from_slice(chunk);
    }

    /// Terminate a final event the server did not close with a blank line.
    fn finish(&mut self) {
        self.buffer.extend_from_slice(b"\n\n");
    }
}

impl Iterator for SseEvents {
    type Item = Value;

    fn next(&mut self) -> Option<Value> {
        loop {
            let end = self
                .buffer
                .windows(2)
                .position(|w| w == b"\n\n")
                .map(|i| (i, 2))
                .into_iter()
                .chain(
                    self.buffer
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|i| (i, 4)),
                )
                .min_by_key(|(i, _)| *i)?;
            let event: Vec<u8> = self.buffer.drain(..end.0 + end.1).collect();
            let event = String::from_utf8_lossy(&event[..end.0]);
            let data: Vec<&str> = event
                .lines()
                .filter_map(|l| l.strip_prefix("data:"))
                .map(|d| d.strip_prefix(' ').unwrap_or(d))
                .collect();
            if let Ok(message) = serde_json::from_str(&data.join("\n")) {
                return Some(message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(chunks: &[&str]) -> Vec<Value> {
        let mut events = SseEvents::default();
        let mut out = Vec::new();
        for chunk in chunks {
            events.push(chunk.as_bytes());
            out.extend(events.by_ref());
        }
        events.finish();
        out.extend(events);
        out
    }

    #[test]
    fn reads_events_split_across_chunks() {
        let got = events(&[
            "event: message\ndata: {\"id\":",
            "1}\n\ndata: {\"id\":2}\n\n",
        ]);
        assert_eq!(got, [json!({"id": 1}), json!({"id": 2})]);
    }

    #[test]
    fn joins_multi_line_data_and_accepts_crlf() {
        let got = events(&["data: {\"id\":2,\r\ndata: \"result\":{}}\r\n\r\n"]);
        assert_eq!(got, [json!({"id": 2, "result": {}})]);
    }

    #[test]
    fn a_final_event_without_a_blank_line_still_counts() {
        assert_eq!(events(&["data: {\"id\":1}"]), [json!({"id": 1})]);
    }

    #[test]
    fn skips_events_that_carry_no_json() {
        let got = events(&[": keepalive\n\nevent: ping\n\ndata: {\"id\":1}\n\n"]);
        assert_eq!(got, [json!({"id": 1})]);
    }

    #[test]
    fn surfaces_the_json_rpc_error_message() {
        let reply = json!({"jsonrpc":"2.0","id":1,"error":{"code":-32602,"message":"no session"}});
        assert_eq!(reply_result(reply).unwrap_err().to_string(), "no session");
    }

    /// Serve one canned response, then hold the connection open as a server
    /// keeping a stream does.
    async fn reply_with(response: Vec<u8>) -> reqwest::Response {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let _ = socket.read(&mut [0; 1024]).await;
            let _ = socket.write_all(&response).await;
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });
        reqwest::get(url).await.unwrap()
    }

    fn with_length(head: &str, body: &str) -> Vec<u8> {
        format!("{head}\r\ncontent-length: {}\r\n\r\n{body}", body.len()).into_bytes()
    }

    /// Reading stops at the reply, so a stream held open afterwards does not
    /// hold the dashboard request.
    #[tokio::test]
    async fn stops_reading_at_the_reply_over_sse() {
        let response = reply_with(
            b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\r\n\
              data: {\"method\":\"notifications/progress\"}\n\n\
              data: {\"id\":2,\"result\":{\"ok\":true}}\n\n"
                .to_vec(),
        )
        .await;
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(5), rpc_result(response, 2))
                .await
                .expect("waited for the stream to end");
        assert_eq!(result.unwrap(), json!({"ok": true}));
    }

    #[tokio::test]
    async fn reads_a_plain_json_reply() {
        let response = reply_with(with_length(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json",
            r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#,
        ))
        .await;
        assert_eq!(rpc_result(response, 1).await.unwrap(), json!({"ok": true}));
    }

    #[tokio::test]
    async fn a_failed_status_is_reported_without_reading_the_body() {
        let response = reply_with(with_length("HTTP/1.1 502 Bad Gateway", "nope")).await;
        assert_eq!(
            rpc_result(response, 1).await.unwrap_err().to_string(),
            "backend returned 502 Bad Gateway"
        );
    }

    #[tokio::test]
    async fn an_oversized_reply_is_refused() {
        let body = format!(r#"{{"id":1,"result":"{}"}}"#, "x".repeat(MAX_REPLY));
        let response = reply_with(with_length(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json",
            &body,
        ))
        .await;
        assert!(rpc_result(response, 1).await.is_err());
    }

    /// Dropping the session wherever the future stopped must still end it at
    /// the backend.
    #[tokio::test]
    async fn dropping_a_session_ends_it_at_the_backend() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp", listener.local_addr().unwrap());
        drop(Session {
            id: Some("abc".into()),
            version: PROTOCOL_VERSION.into(),
            teardown: Some(
                reqwest::Client::new()
                    .delete(url)
                    .header(SESSION_HEADER, "abc"),
            ),
        });

        let (mut socket, _) = listener.accept().await.unwrap();
        let mut buf = vec![0; 1024];
        let n = socket.read(&mut buf).await.unwrap();
        let request = String::from_utf8_lossy(&buf[..n]).to_lowercase();
        let _ = socket.write_all(b"HTTP/1.1 204 No Content\r\n\r\n").await;
        assert!(request.starts_with("delete /mcp "), "{request}");
        assert!(request.contains("mcp-session-id: abc"), "{request}");
    }

    #[test]
    fn pulls_text_out_of_tool_content() {
        let result = json!({"content": [{"type": "text", "text": "hello"}]});
        assert_eq!(tool_text(&result).as_deref(), Some("hello"));
        assert_eq!(tool_text(&json!({"content": []})), None);
    }
}
