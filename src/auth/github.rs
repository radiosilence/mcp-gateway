//! Upstream identity via GitHub OAuth. We never store GitHub credentials; we
//! only use it to answer "who is this human", then key everything on
//! `github:<id>`.

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::config::Config;

const AUTHORIZE_URL: &str = "https://github.com/login/oauth/authorize";
const TOKEN_URL: &str = "https://github.com/login/oauth/access_token";
const USER_URL: &str = "https://api.github.com/user";

/// Build the GitHub authorize URL to redirect the browser to.
pub fn authorize_url(config: &Config, state: &str) -> String {
    let mut url = url::Url::parse(AUTHORIZE_URL).expect("valid url");
    url.query_pairs_mut()
        .append_pair("client_id", &config.github_client_id)
        .append_pair("redirect_uri", &config.github_redirect_uri())
        .append_pair("scope", "read:user")
        .append_pair("state", state)
        .append_pair("allow_signup", "false");
    url.to_string()
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Deserialize)]
struct GithubUser {
    id: u64,
    login: String,
}

/// Exchange an authorization code for the caller's stable subject id
/// (`github:<numeric id>`) and GitHub login.
pub async fn exchange_code(
    config: &Config,
    http: &reqwest::Client,
    code: &str,
) -> Result<(String, String)> {
    let token: TokenResponse = http
        .post(TOKEN_URL)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", config.github_client_id.as_str()),
            ("client_secret", config.github_client_secret.as_str()),
            ("code", code),
            ("redirect_uri", &config.github_redirect_uri()),
        ])
        .send()
        .await
        .context("github token exchange")?
        .error_for_status()
        .context("github token exchange status")?
        .json()
        .await
        .context("parsing github token response")?;

    let user: GithubUser = http
        .get(USER_URL)
        .header("Authorization", format!("Bearer {}", token.access_token))
        .header("User-Agent", "mcp-gateway")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .context("github userinfo")?
        .error_for_status()
        .context("github userinfo status")?
        .json()
        .await
        .context("parsing github user")?;

    Ok((format!("github:{}", user.id), user.login))
}
