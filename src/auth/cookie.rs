//! Cookies carry only opaque ids — the corresponding state lives server-side
//! (see [`crate::store`]). No signed/JWT cookies, nothing forgeable or
//! claim-bearing on the client. Any pod resolves any cookie via the DB.
//! - `fmmcp_session`: dashboard login → `sessions` row.
//! - `fmmcp_oauth`: transient CSRF/login flow → `oauth_flows` row.

pub const SESSION_COOKIE: &str = "fmmcp_session";
pub const OAUTH_COOKIE: &str = "fmmcp_oauth";

/// Build a hardened `Set-Cookie` value.
pub fn set_cookie(name: &str, value: &str, max_age_secs: i64) -> String {
    format!("{name}={value}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={max_age_secs}")
}

/// Build a `Set-Cookie` that clears the named cookie.
pub fn clear_cookie(name: &str) -> String {
    format!("{name}=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0")
}

/// Pull a cookie value out of a raw `Cookie` header.
pub fn read_cookie<'a>(cookie_header: &'a str, name: &str) -> Option<&'a str> {
    cookie_header.split(';').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k.trim() == name).then(|| v.trim())
    })
}
