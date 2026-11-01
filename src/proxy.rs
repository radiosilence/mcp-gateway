//! Streaming reverse proxy: `/{id}` → the backend MCP for `{id}`.
//!
//! Per request: introspect the bearer at Hydra to get the subject, decrypt the
//! subject's credentials for this MCP, replace any client-supplied copy of
//! those headers with the stored values, forward, and stream the response back
//! (streamable-HTTP responses can be SSE).
//!
//! No valid bearer gets a 401 with an RFC 9728 `WWW-Authenticate`, which is
//! what starts a client's OAuth flow.

use axum::body::{Body, Bytes};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use crate::state::AppState;

const HOP_BY_HOP: [&str; 8] = [
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailers",
    "transfer-encoding",
    "upgrade",
];

pub async fn handle(
    State(state): State<AppState>,
    Path(id): Path<String>,
    method: Method,
    uri: Uri,
    mut headers: HeaderMap,
    body: Bytes,
) -> Response {
    let Some(mcp) = state.config.mcp(&id) else {
        return (StatusCode::NOT_FOUND, "unknown mcp").into_response();
    };

    let Some(token) = bearer(&headers).map(str::to_owned) else {
        return challenge(&state, &id);
    };
    let sub = match state.hydra.introspect(&token).await {
        Ok(Some(sub)) => sub,
        Ok(None) => return challenge(&state, &id),
        // A 401 would send the client back through OAuth for an outage that a
        // new token cannot fix.
        Err(e) => {
            tracing::error!(error = format!("{e:#}"), "token introspection failed");
            return unavailable("authorization server unavailable");
        }
    };

    let credentials = match mcp.public {
        true => None,
        false => match state
            .store
            .get_credentials(&sub, &id, mcp.primary_field())
            .await
        {
            Ok(credentials) => credentials,
            Err(e) => {
                tracing::error!(error = format!("{e:#}"), mcp = %id, "credential lookup failed");
                return unavailable("credential store unavailable");
            }
        },
    };
    if credentials.is_none() && !mcp.public {
        // Forwarded anyway: the backend answers with its own "not configured"
        // error, which the model can relay to the user.
        tracing::debug!(%sub, mcp = %id, "no credential stored");
    }

    // The session cookie must not reach backends, and a backend's Set-Cookie
    // must not land on the gateway's origin; MCP sessions use `mcp-session-id`.
    headers.remove(header::AUTHORIZATION);
    headers.remove(header::HOST);
    headers.remove(header::CONTENT_LENGTH);
    headers.remove(header::COOKIE);
    for h in HOP_BY_HOP {
        headers.remove(h);
    }
    for field in &mcp.fields {
        headers.remove(&field.header);
        let Some(value) = credentials
            .as_ref()
            .and_then(|c| c.get(&field.id))
            .filter(|v| !v.is_empty())
        else {
            continue;
        };
        match HeaderValue::from_str(value) {
            Ok(mut v) => {
                v.set_sensitive(true);
                headers.insert(field.header.clone(), v);
            }
            Err(_) => tracing::warn!(
                mcp = %id,
                field = %field.id,
                "stored credential is not a valid header value; skipping"
            ),
        }
    }

    let url = match uri.query() {
        Some(query) if mcp.backend.contains('?') => format!("{}&{query}", mcp.backend),
        Some(query) => format!("{}?{query}", mcp.backend),
        None => mcp.backend.clone(),
    };
    let mut upstream = match state
        .proxy_http
        .request(method, url)
        .headers(headers)
        .body(body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, mcp = %id, "backend request failed");
            return (StatusCode::BAD_GATEWAY, "backend unavailable").into_response();
        }
    };

    let status = upstream.status();
    let mut resp_headers = std::mem::take(upstream.headers_mut());
    resp_headers.remove(header::CONTENT_LENGTH);
    resp_headers.remove(header::SET_COOKIE);
    for h in HOP_BY_HOP {
        resp_headers.remove(h);
    }

    let mut out = Response::new(Body::from_stream(upstream.bytes_stream()));
    *out.status_mut() = status;
    *out.headers_mut() = resp_headers;
    out
}

/// The auth scheme is case-insensitive (RFC 9110 §11.1).
fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    let token = token.trim();
    (scheme.eq_ignore_ascii_case("bearer") && !token.is_empty()).then_some(token)
}

fn challenge(state: &AppState, id: &str) -> Response {
    let metadata_url = crate::well_known::metadata_url(state, id);
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            format!("Bearer resource_metadata=\"{metadata_url}\""),
        )],
    )
        .into_response()
}

fn unavailable(message: &'static str) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [(header::RETRY_AFTER, "5")],
        message,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn auth(value: &str) -> HeaderMap {
        HeaderMap::from_iter([(header::AUTHORIZATION, HeaderValue::from_str(value).unwrap())])
    }

    #[test]
    fn reads_the_bearer_token_whatever_the_scheme_case() {
        assert_eq!(bearer(&auth("Bearer abc")), Some("abc"));
        assert_eq!(bearer(&auth("bearer abc")), Some("abc"));
        assert_eq!(bearer(&auth("Basic abc")), None);
        assert_eq!(bearer(&auth("Bearer ")), None);
        assert_eq!(bearer(&HeaderMap::new()), None);
    }
}
