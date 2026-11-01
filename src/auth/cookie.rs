//! Cookies carry only opaque ids; the state they name lives in the database.
//!
//! `__Host-` makes the browser refuse them unless they are `Secure`, scoped to
//! `/` and carry no `Domain`, so a sibling subdomain cannot set or shadow them.

pub const SESSION_COOKIE: &str = "__Host-session";
pub const OAUTH_COOKIE: &str = "__Host-oauth";
/// Names used before the `__Host-` prefix, cleared wherever a session changes.
pub const LEGACY_COOKIES: [&str; 2] = ["fmmcp_session", "fmmcp_oauth"];

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
