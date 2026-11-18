use anyhow::{anyhow, Result};
use reqwest::Client;
use serde_json::json;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    let client = Client::builder()
        .cookie_store(true)
        .danger_accept_invalid_certs(true)
        .build()?;

    // 1. Authorize GET to fetch CSRF and session cookies
    let auth_url = "https://iam.avrig.ai/auth/v1/oidc/authorize?client_id=rauthy&response_type=code&redirect_uri=https://iam.avrig.ai/auth/v1/oidc/callback&scope=openid&code_challenge=E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM&code_challenge_method=S256";
    let get_res = client.get(auth_url).send().await?;

    println!("GET headers: {:?}", get_res.headers());
    let cookies: Vec<_> = get_res.headers().get_all("set-cookie").iter().map(|v| v.to_str().unwrap().to_string()).collect();
    println!("Set-Cookie headers: {:?}", cookies);

    let html = get_res.text().await?;

    let csrf = regex::Regex::new(r#"id="tpl_csrf_token">([^<]+)<"#)?
        .captures(&html)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| anyhow!("CSRF token not found"))?;

    println!("Fetched CSRF token: {}", csrf);

    // 2. Fetch POW challenge
    let pow_challenge = client.post("https://iam.avrig.ai/auth/v1/pow")
        .send()
        .await?
        .text()
        .await?;
    println!("POW challenge: {}", pow_challenge);

    // 3. Solve POW
    let pow_solution = spow::wasm::pow_work(&pow_challenge)
        .map_err(|e| anyhow!("solving pow: {}", e))?;
    println!("Solved POW: {}", pow_solution);

    // 4. Read admin password from WSL secrets file
    let password = std::fs::read_to_string(r"\\wsl.localhost\avrig-ai\root\avrig-ai-secrets\rauthy-admin-password")
        .or_else(|_| std::fs::read_to_string(r"\\wsl$\avrig-ai\root\avrig-ai-secrets\rauthy-admin-password"))
        .unwrap_or_else(|_| "password.123".into());
    let password = password.trim();

    // 5. Submit Login via POST /auth/v1/oidc/authorize
    println!("Submitting login for wirianto@avrig.ai...");
    let login_payload = json!({
        "client_id": "rauthy",
        "redirect_uri": "https://iam.avrig.ai/auth/v1/oidc/callback",
        "email": "wirianto@avrig.ai",
        "password": password,
        "pow": pow_solution,
        "csrf_token": csrf,
    });

    let res = client.post("https://iam.avrig.ai/auth/v1/oidc/authorize")
        .json(&login_payload)
        .send()
        .await?;

    println!("Login status: {}", res.status());
    let body = res.text().await?;
    println!("Login response body:\n{}", body);

    Ok(())
}
