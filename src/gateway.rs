use avrig_gateway::LlmProviderClient;
use avrig_gateway::antigravity::converter::ToolHistoryValidationError;
use avrig_gateway::antigravity::quota::fetch_gemini_cli_quota;
use avrig_gateway::client::UpstreamStatusError;
use avrig_gateway::resolver::{ModelResolutionError, resolve_upstream_model_id};
use avrig_gateway::types::ChatRequest;
use axum::{
    Json, Router,
    extract::{ConnectInfo, State},
    http::{HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{
        IntoResponse, Response,
        sse::{Event as SseEvent, Sse},
    },
    routing::{get, post},
};
use futures_util::StreamExt;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;

#[derive(Clone)]
pub struct GatewayState {
    pub client: Arc<LlmProviderClient>,
    pub local_only: bool,
    pub pending_oauth: Arc<tokio::sync::RwLock<std::collections::HashMap<String, String>>>,
}

pub fn build_gateway_router(state: GatewayState) -> Router {
    let local_only = state.local_only;
    let mut router = Router::new()
        .route("/v1/health", get(handle_health))
        .route("/api/v1/health", get(handle_health))
        .route("/health", get(handle_health))
        .route("/v1/version", get(handle_version))
        .route("/api/v1/version", get(handle_version))
        .route("/version", get(handle_version))
        .route("/v1/models", get(handle_models))
        .route("/api/v1/models", get(handle_models))
        .route("/v1/quota", get(handle_quota))
        .route("/api/v1/quota", get(handle_quota))
        .route("/v1/auth/status", get(handle_auth_status))
        .route("/v1/auth/antigravity/login", get(handle_antigravity_login))
        .route("/oauth-callback", get(handle_antigravity_callback))
        .route("/v1/chat/completions", post(handle_chat_completions));

    if local_only {
        router = router.layer(middleware::from_fn(local_only_guard));
    }

    router.with_state(Arc::new(state))
}

pub async fn local_only_guard(
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    req: axum::extract::Request,
    next: Next,
) -> Result<Response, (StatusCode, &'static str)> {
    if !addr.ip().is_loopback() {
        return Err((StatusCode::FORBIDDEN, "localhost clients only"));
    }
    Ok(next.run(req).await)
}

/// Target triple reported by build metadata.
///
/// `build.rs` captures `TARGET` (for example `x86_64-pc-windows-msvc`). The `OS`/`ARCH`
/// pair remains as a fallback for any build that skipped the build script.
pub fn target_triple() -> String {
    match option_env!("AVRIG_TARGET_TRIPLE") {
        Some(triple) if !triple.is_empty() && triple != "unknown" => triple.to_string(),
        _ => format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
    }
}

/// Build metadata exposed by the health endpoint, captured at compile time by `build.rs`.
///
/// Commit identity, build timestamp, and dirtiness come from the build script. Each field
/// degrades to `unknown` (or `false` for `dirty`) when the build ran without git metadata.
pub fn build_metadata() -> serde_json::Value {
    json!({
        "commit_short_sha": option_env!("AVRIG_COMMIT_SHORT_SHA").unwrap_or("unknown"),
        "build_timestamp": option_env!("AVRIG_BUILD_TIMESTAMP").unwrap_or("unknown"),
        "target_triple": target_triple(),
        "dirty": matches!(option_env!("AVRIG_GIT_DIRTY"), Some("true"))
    })
}

pub async fn handle_health(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let accounts = state.client.list_accounts().await;
    let active_providers: Vec<String> = accounts.iter().map(|a| a.provider.clone()).collect();
    Json(json!({
        "ok": true,
        "providers": active_providers,
        "accounts_count": accounts.len(),
        "version": env!("CARGO_PKG_VERSION"),
        "semver": env!("CARGO_PKG_VERSION"),
        "build": build_metadata()
    }))
}

pub async fn handle_version() -> impl IntoResponse {
    let raw_semver = env!("CARGO_PKG_VERSION");
    let mut parts = raw_semver.split('.');
    let major = parts.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let minor = parts.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
    let patch = parts.next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);

    Json(json!({
        "semver": raw_semver,
        "version": raw_semver,
        "major": major,
        "minor": minor,
        "patch": patch,
        "build": build_metadata()
    }))
}

pub async fn handle_auth_status(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let accounts = state.client.list_accounts().await;
    Json(json!({
        "ok": true,
        "accounts": accounts
    }))
}

pub async fn handle_models(State(state): State<Arc<GatewayState>>) -> Response {
    match state.client.list_models().await {
        Ok(models) => Json(json!({
            "object": "list",
            "data": models
        }))
        .into_response(),
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({
                "error": {
                    "type": "model_discovery_error",
                    "message": err.to_string()
                }
            })),
        )
            .into_response(),
    }
}

pub async fn handle_quota(State(state): State<Arc<GatewayState>>) -> Response {
    let ags = state.client.antigravity_accounts.read().await;
    if let Some(first) = ags.first() {
        let creds = first.state.creds.read().await.clone();
        match fetch_gemini_cli_quota(&first.state.http, &creds).await {
            Ok(quota) => Json(quota).into_response(),
            Err(err) => (
                StatusCode::BAD_GATEWAY,
                Json(json!({
                    "error": {
                        "type": "quota_error",
                        "message": err.to_string()
                    }
                })),
            )
                .into_response(),
        }
    } else {
        (
            StatusCode::NOT_FOUND,
            Json(json!({
                "error": {
                    "type": "no_quota_provider",
                    "message": "Antigravity provider is not enabled"
                }
            })),
        )
            .into_response()
    }
}

pub async fn handle_chat_completions(
    State(state): State<Arc<GatewayState>>,
    headers: HeaderMap,
    Json(mut input): Json<ChatRequest>,
) -> Response {
    input.session_id = session_header_value(&headers);
    input.user_agent = header_value(&headers, "user-agent");
    let started = std::time::Instant::now();
    let requested_model = input.model.trim().to_string();
    let provider = state.client.resolve_provider_name(&requested_model).await;
    let resolved_model = resolve_upstream_model_id(&requested_model);
    let stream_requested = input.stream == Some(true);

    if stream_requested {
        match state.client.chat_completion_stream(input).await {
            Ok(stream) => {
                tracing::info!(
                    provider = %provider,
                    requested_model = %requested_model,
                    upstream_model = %resolved_model,
                    upstream_status = ?Some(200_u16),
                    stream = stream_requested,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "chat completions stream started"
                );
                let stream_provider = provider.clone();
                let stream_model = requested_model.clone();
                let mapped = stream
                    .map(move |res| match res {
                        Ok(chunk) => {
                            let data = serde_json::to_string(&chunk).unwrap_or_default();
                            Ok::<SseEvent, std::convert::Infallible>(SseEvent::default().data(data))
                        }
                        Err(e) => {
                            tracing::warn!(
                                provider = %stream_provider,
                                requested_model = %stream_model,
                                upstream_status = ?upstream_status(&e),
                                elapsed_ms = started.elapsed().as_millis() as u64,
                                error = %e,
                                "chat completions stream failed"
                            );
                            let err_json = json!({
                                "error": {
                                    "message": e.to_string(),
                                    "type": "upstream_error"
                                }
                            })
                            .to_string();
                            Ok::<SseEvent, std::convert::Infallible>(
                                SseEvent::default().data(err_json),
                            )
                        }
                    })
                    .chain(futures_util::stream::once(async {
                        Ok::<SseEvent, std::convert::Infallible>(SseEvent::default().data("[DONE]"))
                    }));

                Sse::new(mapped).into_response()
            }
            Err(err) => {
                let (status, error_type) = chat_completion_error_status(&err);
                tracing::warn!(
                    provider = %provider,
                    requested_model = %requested_model,
                    upstream_model = %resolved_model,
                    upstream_status = ?upstream_status(&err),
                    http_status = status.as_u16(),
                    error_type = error_type,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    error = %err,
                    "chat completions request rejected"
                );
                chat_completion_error_response(err)
            }
        }
    } else {
        match state.client.chat_completion(input).await {
            Ok(resp) => {
                tracing::info!(
                    provider = %provider,
                    requested_model = %requested_model,
                    upstream_model = %resolved_model,
                    response_model = %resp.model,
                    stream = stream_requested,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    "chat completions request completed"
                );
                Json(resp).into_response()
            }
            Err(err) => {
                let (status, error_type) = chat_completion_error_status(&err);
                tracing::warn!(
                    provider = %provider,
                    requested_model = %requested_model,
                    upstream_model = %resolved_model,
                    upstream_status = ?upstream_status(&err),
                    http_status = status.as_u16(),
                    error_type = error_type,
                    elapsed_ms = started.elapsed().as_millis() as u64,
                    error = %err,
                    "chat completions request failed"
                );
                chat_completion_error_response(err)
            }
        }
    }
}

/// Upstream status carried by the error, when the provider reported one.
fn upstream_status(err: &anyhow::Error) -> Option<u16> {
    err.downcast_ref::<UpstreamStatusError>().map(|e| e.status)
}

/// Non-empty, trimmed value of one inbound header.
fn header_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .map(str::to_owned)
}

/// Session id supplied by the calling client, if any.
///
/// OpenCode sends `x-opencode-session` on every model request and OpenCode Go requires it.
/// `x-opencode-session-id` and `X-Session-Id` are accepted as fallbacks for other clients.
fn session_header_value(headers: &HeaderMap) -> Option<String> {
    [
        "x-opencode-session",
        "x-opencode-session-id",
        "X-Session-Id",
    ]
    .iter()
    .find_map(|name| header_value(headers, name))
}

/// Map a dispatch failure to its HTTP status and error envelope type.
fn chat_completion_error_status(err: &anyhow::Error) -> (StatusCode, &'static str) {
    if err.downcast_ref::<ToolHistoryValidationError>().is_some() {
        (StatusCode::BAD_REQUEST, "invalid_request_error")
    } else if err.downcast_ref::<ModelResolutionError>().is_some() {
        (StatusCode::NOT_FOUND, "model_not_found")
    } else if err
        .downcast_ref::<UpstreamStatusError>()
        .is_some_and(UpstreamStatusError::reports_unknown_model)
    {
        // Upstream 400/404 that names the model as unknown: the caller's model is wrong, so
        // report it like a local resolution failure instead of an upstream outage.
        (StatusCode::NOT_FOUND, "model_not_found")
    } else {
        (StatusCode::BAD_GATEWAY, "upstream_error")
    }
}

fn chat_completion_error_response(err: anyhow::Error) -> Response {
    let (status, error_type) = chat_completion_error_status(&err);
    (
        status,
        Json(json!({
            "error": {
                "type": error_type,
                "message": err.to_string()
            }
        })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use avrig_gateway::antigravity::converter::ToolHistoryValidationError;

    #[test]
    fn every_tool_history_validation_variant_maps_to_bad_request() {
        // All of these are Claude-path rejections: they must fail before dispatch, not turn
        // into an upstream 502.
        let cases = [
            ToolHistoryValidationError::EmptyCallId,
            ToolHistoryValidationError::DuplicateCallId("call_dup".into()),
            ToolHistoryValidationError::MissingResultCallId,
            ToolHistoryValidationError::InvalidResult("call_orphan".into()),
            ToolHistoryValidationError::UnmatchedCalls(1),
        ];
        for case in cases {
            let label = case.to_string();
            let response = chat_completion_error_response(case.into());
            assert_eq!(
                response.status(),
                StatusCode::BAD_REQUEST,
                "variant {label} must map to 400"
            );
        }
    }

    #[tokio::test]
    async fn tool_history_validation_body_reports_invalid_request() {
        let response = chat_completion_error_response(
            ToolHistoryValidationError::InvalidResult("x".into()).into(),
        );
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("read body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(
            value["error"]["type"],
            serde_json::json!("invalid_request_error")
        );
    }

    #[test]
    fn non_validation_failure_maps_to_bad_gateway() {
        let response = chat_completion_error_response(anyhow::anyhow!("provider unavailable"));
        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[test]
    fn unknown_model_prefix_maps_to_not_found() {
        let response =
            chat_completion_error_response(ModelResolutionError::ModelNotFound {
                prefix: "composync".into(),
                model: "composync/paid".into(),
            }
            .into());
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn unknown_model_prefix_body_reports_model_not_found() {
        let response =
            chat_completion_error_response(ModelResolutionError::ModelNotFound {
                prefix: "composync".into(),
                model: "composync/paid".into(),
            }
            .into());
        let body = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .expect("read body");
        let value: serde_json::Value = serde_json::from_slice(&body).expect("json body");
        assert_eq!(value["error"]["type"], serde_json::json!("model_not_found"));
        assert_eq!(
            value["error"]["message"],
            serde_json::json!("Unknown model prefix 'composync' for model 'composync/paid'")
        );
    }

    #[test]
    fn unlisted_sub_model_maps_to_not_found() {
        let response =
            chat_completion_error_response(ModelResolutionError::SubModelNotFound {
                provider: "composync".into(),
                model: "composync/does-not-exist-model".into(),
            }
            .into());
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn upstream_unknown_model_fault_maps_to_not_found() {
        // The live composync 400 for a nonexistent sub-model must be reported as model_not_found.
        let err = UpstreamStatusError {
            status: 400,
            body: r#"{"error":{"message":"Unable to determine provider for model 'does-not-exist-model'.","type":"invalid_request_error","code":"bad_request"}}"#.to_string(),
        };
        let (status, error_type) = chat_completion_error_status(&err.into());
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(error_type, "model_not_found");
    }

    #[test]
    fn generic_upstream_bad_requests_stay_bad_gateway() {
        for body in [
            r#"{"error":{"message":"messages: at least one message is required"}}"#,
            r#"{"error":{"message":"This model's maximum context length is 8192 tokens, however you requested 9000 tokens."}}"#,
        ] {
            let err = UpstreamStatusError {
                status: 400,
                body: body.to_string(),
            };
            let (status, error_type) = chat_completion_error_status(&err.into());
            assert_eq!(status, StatusCode::BAD_GATEWAY, "body: {body}");
            assert_eq!(error_type, "upstream_error", "body: {body}");
        }
    }

    #[test]
    fn claude_path_rejection_maps_to_bad_request_before_dispatch() {
        // The variant table above proves the mapping. This test proves the Claude replay path
        // itself produces a validation error for a real history shape: the paired history
        // validates, while the same history without its tool result is rejected locally rather
        // than reaching upstream and surfacing as a 502.
        let paired: ChatRequest = serde_json::from_value(json!({
            "model": "agy/claude-opus-5-5-high",
            "messages": [
                { "role": "user", "content": "weather?" },
                {
                    "role": "assistant",
                    "content": null,
                    "tool_calls": [{
                        "id": "call_claude_handler_1",
                        "type": "function",
                        "function": { "name": "lookup", "arguments": "{}" }
                    }]
                },
                { "role": "tool", "tool_call_id": "call_claude_handler_1", "content": "{}" }
            ]
        }))
        .expect("build paired Claude request");
        avrig_gateway::antigravity::converter::validate_tool_history(&paired.messages)
            .expect("paired Claude replay must validate");

        let mut unpaired = paired.clone();
        unpaired.messages.pop();
        let err = avrig_gateway::antigravity::converter::validate_tool_history(&unpaired.messages)
            .expect_err("unpaired Claude replay must be rejected");
        assert!(
            matches!(err, ToolHistoryValidationError::UnmatchedCalls(1)),
            "unexpected variant: {err:?}"
        );

        let response = chat_completion_error_response(err.into());
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn health_reports_version_and_build_metadata() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = GatewayState {
            client: Arc::new(LlmProviderClient::new()),
            local_only: false,
            pending_oauth: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        };
        let router = build_gateway_router(state);

        for path in ["/health", "/api/v1/health", "/v1/health"] {
            let req = Request::builder()
                .uri(path)
                .method("GET")
                .body(Body::empty())
                .expect("build health request");
            let response = router.clone().oneshot(req).await.expect("health response");
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("read health body");
            let value: serde_json::Value =
                serde_json::from_slice(&body).expect("health body is json");
            println!("{path} -> {value}");

            assert_eq!(value["ok"], serde_json::json!(true));
            assert!(value["providers"].is_array());
            assert!(value["accounts_count"].is_number());
            assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
            assert_eq!(value["semver"], env!("CARGO_PKG_VERSION"));

            let build = &value["build"];
            assert_eq!(build["target_triple"], target_triple());
            assert!(build["dirty"].is_boolean());

            let commit = build["commit_short_sha"]
                .as_str()
                .expect("commit_short_sha is a string");
            if commit != "unknown" {
                assert_eq!(commit.len(), 7, "short sha must be 7 characters");
            }

            let timestamp = build["build_timestamp"]
                .as_str()
                .expect("build_timestamp is a string");
            if timestamp != "unknown" {
                assert_eq!(timestamp.len(), 20, "ISO-8601 UTC timestamp is 20 chars");
                assert!(timestamp.ends_with('Z'), "timestamp must be UTC");
            }
        }
    }

    #[tokio::test]
    async fn version_reports_its_own_build_object_matching_health() {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use tower::ServiceExt;

        let state = GatewayState {
            client: Arc::new(LlmProviderClient::new()),
            local_only: false,
            pending_oauth: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
        };
        let router = build_gateway_router(state);

        let mut health_build: Option<serde_json::Value> = None;
        for path in ["/health", "/api/v1/health", "/v1/health"] {
            let req = Request::builder()
                .uri(path)
                .method("GET")
                .body(Body::empty())
                .expect("build health request");
            let response = router.clone().oneshot(req).await.expect("health response");
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("read health body");
            let value: serde_json::Value =
                serde_json::from_slice(&body).expect("health body is json");
            health_build = Some(value["build"].clone());
        }

        for path in ["/version", "/api/v1/version", "/v1/version"] {
            let req = Request::builder()
                .uri(path)
                .method("GET")
                .body(Body::empty())
                .expect("build version request");
            let response = router.clone().oneshot(req).await.expect("version response");
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .expect("read version body");
            let value: serde_json::Value =
                serde_json::from_slice(&body).expect("version body is json");
            println!("{path} -> {value}");

            assert_eq!(value["semver"], env!("CARGO_PKG_VERSION"));
            assert_eq!(value["version"], env!("CARGO_PKG_VERSION"));
            assert!(value["major"].is_number());
            assert!(value["minor"].is_number());
            assert!(value["patch"].is_number());

            // V4 and V5 both read the build object, so the two endpoints must not disagree.
            let build = &value["build"];
            assert_eq!(
                build,
                &health_build.clone().expect("health build captured"),
                "{path} must report the same build object as /api/v1/health"
            );
            assert_eq!(build["target_triple"], target_triple());
            assert!(build["dirty"].is_boolean());
            assert!(
                build["commit_short_sha"].is_string(),
                "commit_short_sha is a string"
            );
            assert!(
                build["build_timestamp"].is_string(),
                "build_timestamp is a string"
            );
        }
    }
}

pub async fn handle_antigravity_login(State(state): State<Arc<GatewayState>>) -> impl IntoResponse {
    let client_id = avrig_gateway::antigravity::types::default_client_id();
    let redirect_uri = "http://127.0.0.1:8000/oauth-callback";

    match avrig_gateway::antigravity::oauth::generate_pkce_and_auth_url(&client_id, redirect_uri) {
        Ok((auth_url, verifier, oauth_state)) => {
            state
                .pending_oauth
                .write()
                .await
                .insert(oauth_state.clone(), verifier);
            Json(json!({
                "ok": true,
                "auth_url": auth_url,
                "state": oauth_state,
                "message": "Open auth_url in your browser. Callback will land on port 8000."
            }))
            .into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": err.to_string() })),
        )
            .into_response(),
    }
}

#[derive(serde::Deserialize)]
pub struct OAuthCallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
}

pub async fn handle_antigravity_callback(
    State(state): State<Arc<GatewayState>>,
    axum::extract::Query(query): axum::extract::Query<OAuthCallbackQuery>,
) -> impl IntoResponse {
    if let Some(err) = query.error {
        return (
            StatusCode::BAD_REQUEST,
            format!("Google authorization error: {}", err),
        )
            .into_response();
    }
    let Some(code) = query.code else {
        return (StatusCode::BAD_REQUEST, "Missing code parameter").into_response();
    };
    let Some(oauth_state) = query.state else {
        return (StatusCode::BAD_REQUEST, "Missing state parameter").into_response();
    };

    let verifier = { state.pending_oauth.write().await.remove(&oauth_state) };
    let Some(verifier) = verifier else {
        return (StatusCode::BAD_REQUEST, "Unknown or expired OAuth state").into_response();
    };

    let client_id = avrig_gateway::antigravity::types::default_client_id();
    let client_secret = avrig_gateway::antigravity::types::default_client_secret();
    let target_path = std::path::PathBuf::from("config/antigravity.json");

    match avrig_gateway::antigravity::oauth::exchange_code(
        &target_path,
        &client_id,
        &client_secret,
        &code,
        &verifier,
    ) {
        Ok(creds) => {
            let ag_state = avrig_gateway::antigravity::state::AppState::new(
                target_path,
                client_id.clone(),
                client_secret.clone(),
                creds.clone(),
            );
            let alias = creds.email.clone().unwrap_or_else(|| "primary".to_string());
            state
                .client
                .add_antigravity_account(alias.clone(), ag_state, 10)
                .await;

            let html = format!(
                "<html><body style='font-family:sans-serif;background:#18181b;color:#f4f4f5;display:flex;align-items:center;justify-content:center;height:100vh;margin:0;'>\
                <div style='background:#27272a;padding:32px;border-radius:12px;border:1px solid #3f3f46;text-align:center;'>\
                <h2 style='color:#10b981;margin-top:0;'>Authorization Successful!</h2>\
                <p>Google Antigravity account <b>{}</b> has been registered in the Avrig Gateway.</p>\
                <p style='color:#a1a1aa;font-size:14px;'>You can now close this tab.</p>\
                </div></body></html>",
                alias
            );
            (
                [(axum::http::header::CONTENT_TYPE, "text/html; charset=utf-8")],
                html,
            )
                .into_response()
        }
        Err(err) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to exchange authorization code: {}", err),
        )
            .into_response(),
    }
}

#[cfg(test)]
mod session_header_tests {
    use super::*;
    use axum::http::HeaderValue;

    /// Build a header map from literal pairs; header names are case-insensitive.
    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.insert(*name, HeaderValue::from_str(value).expect("header value"));
        }
        map
    }

    #[test]
    fn canonical_session_header_wins_when_all_are_present() {
        let map = headers(&[
            ("x-opencode-session", "canonical"),
            ("x-opencode-session-id", "session-id"),
            ("X-Session-Id", "session-affinity"),
        ]);
        assert_eq!(session_header_value(&map).as_deref(), Some("canonical"));
    }

    #[test]
    fn opencode_session_id_is_used_alone() {
        let map = headers(&[("x-opencode-session-id", "abc-123")]);
        assert_eq!(session_header_value(&map).as_deref(), Some("abc-123"));
    }

    #[test]
    fn session_affinity_header_is_used_alone() {
        let map = headers(&[("X-Session-Id", "affinity-9")]);
        assert_eq!(session_header_value(&map).as_deref(), Some("affinity-9"));
    }

    #[test]
    fn blank_candidate_is_skipped_for_the_next_one() {
        let map = headers(&[
            ("x-opencode-session", "   "),
            ("x-opencode-session-id", "  next-value  "),
        ]);
        assert_eq!(session_header_value(&map).as_deref(), Some("next-value"));
    }

    #[test]
    fn no_session_header_yields_none() {
        let map = headers(&[("authorization", "Bearer token")]);
        assert_eq!(session_header_value(&map), None);
        assert_eq!(session_header_value(&HeaderMap::new()), None);
    }
}
