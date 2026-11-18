use avrig_gateway::LlmProviderClient;
use avrig_gateway::resolver::{is_siwc_model, resolve_upstream_model_id, strip_model_prefix};
use avrig_gateway::types::{ChatMessage, ChatRequest};
use std::sync::Arc;

#[test]
fn test_model_resolver() {
    assert_eq!(
        strip_model_prefix("agy/gemini-2.5-flash"),
        (Some("agy"), "gemini-2.5-flash".to_string())
    );
    assert_eq!(
        strip_model_prefix("chatgpt/gpt-4o"),
        (Some("chatgpt"), "gpt-4o".to_string())
    );
    assert_eq!(
        strip_model_prefix("gemini-2.5-flash"),
        (None, "gemini-2.5-flash".to_string())
    );

    assert!(is_siwc_model("chatgpt/gpt-4o"));
    assert!(is_siwc_model("gpt-4o"));
    assert!(is_siwc_model("o3-mini"));
    assert!(!is_siwc_model("agy/gemini-2.5-flash"));
    assert!(!is_siwc_model("gemini-2.5-flash"));

    assert_eq!(
        resolve_upstream_model_id("agy/default"),
        "gemini-3.8-flash-tiered"
    );
    assert_eq!(resolve_upstream_model_id("chatgpt/default"), "gpt-5.6-luna");
}

#[tokio::test]
async fn test_llm_provider_client_registry() {
    let client = LlmProviderClient::new();
    client
        .add_direct_provider(
            "test_direct".to_string(),
            "http://127.0.0.1:12345".to_string(),
            vec!["key1".to_string(), "key2".to_string()],
            None,
            None,
        )
        .await;

    let accounts = client.list_accounts().await;
    assert_eq!(accounts.len(), 1);
    assert_eq!(accounts[0].provider, "test_direct");
    assert_eq!(accounts[0].healthy, true);
}

#[tokio::test]
async fn test_gateway_route_handlers() {
    let client = Arc::new(LlmProviderClient::new());
    let state = avrig::gateway::GatewayState {
        client,
        local_only: false,
        pending_oauth: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
    };
    let router = avrig::gateway::build_gateway_router(state);

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    // 1. GET /v1/health & aliases
    for health_path in ["/v1/health", "/api/v1/health", "/health"] {
        let req = Request::builder()
            .uri(health_path)
            .method("GET")
            .body(Body::empty())
            .unwrap();
        let resp = router.clone().oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // 2. GET /v1/models
    let req = Request::builder()
        .uri("/v1/models")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 3. GET /v1/auth/status
    let req = Request::builder()
        .uri("/v1/auth/status")
        .method("GET")
        .body(Body::empty())
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // 4. POST /v1/chat/completions (with no active account pool, expect bad gateway or error envelope)
    let chat_req = ChatRequest {
        model: "agy/gemini-2.5-flash".into(),
        messages: vec![ChatMessage {
            role: "user".into(),
            content: Some(serde_json::json!("hello")),
            tool_calls: None,
            tool_call_id: None,
        }],
        stream: None,
        stream_options: None,
        temperature: None,
        top_p: None,
        max_tokens: None,
        max_completion_tokens: None,
        max_output_tokens: None,
        tools: None,
        tool_choice: None,
        session_id: None,
        user_agent: None,
    };
    let req = Request::builder()
        .uri("/v1/chat/completions")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&chat_req).unwrap()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
}

#[tokio::test]
async fn test_unknown_model_prefix_is_rejected_before_dispatch() {
    let client = Arc::new(LlmProviderClient::new());
    let state = avrig::gateway::GatewayState {
        client,
        local_only: false,
        pending_oauth: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
    };
    let router = avrig::gateway::build_gateway_router(state);

    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    // A prefix that names no provider and no built-in backend must be rejected locally,
    // not dispatched to whichever provider happens to be configured.
    let chat_req = ChatRequest {
        model: "composync/paid".into(),
        messages: vec![ChatMessage {
            role: "user".into(),
            content: Some(serde_json::json!("hello")),
            tool_calls: None,
            tool_call_id: None,
        }],
        stream: None,
        stream_options: None,
        temperature: None,
        top_p: None,
        max_tokens: None,
        max_completion_tokens: None,
        max_output_tokens: None,
        tools: None,
        tool_choice: None,
        session_id: None,
        user_agent: None,
    };
    let req = Request::builder()
        .uri("/v1/chat/completions")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_vec(&chat_req).unwrap()))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"]["type"], serde_json::json!("model_not_found"));
    assert_eq!(
        value["error"]["message"],
        serde_json::json!("Unknown model prefix 'composync' for model 'composync/paid'")
    );
}

/// Minimal OpenAI-compatible stub provider for the direct-provider routes.
///
/// `/models` advertises one model and any `/chat/completions` request answers with a fixed
/// completion, which is enough to exercise catalogue validation end to end.
async fn spawn_stub_provider() -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind stub provider");
    let addr = listener.local_addr().expect("stub provider address");
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = vec![0u8; 4096];
                let Ok(read) = socket.read(&mut buf).await else {
                    return;
                };
                let head = String::from_utf8_lossy(&buf[..read]).to_string();
                let body = if head.starts_with("GET /models") {
                    r#"{"object":"list","data":[{"id":"known-model"}]}"#
                } else {
                    r#"{"id":"chatcmpl-stub","object":"chat.completion","created":0,"model":"known-model","choices":[],"usage":{"prompt_tokens":0,"completion_tokens":0,"total_tokens":0}}"#
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
            });
        }
    });
    format!("http://{}", addr)
}

async fn stub_provider_router(base_url: String) -> axum::Router {
    let client = Arc::new(LlmProviderClient::new());
    client
        .add_direct_provider(
            "composync".to_string(),
            base_url,
            vec!["test-key".to_string()],
            None,
            None,
        )
        .await;
    avrig::gateway::build_gateway_router(avrig::gateway::GatewayState {
        client,
        local_only: false,
        pending_oauth: Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new())),
    })
}

fn chat_request(model: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "model": model,
        "messages": [{ "role": "user", "content": "ping" }]
    }))
    .expect("serialize chat request")
}

#[tokio::test]
async fn test_unknown_sub_model_under_direct_provider_maps_to_not_found() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    let router = stub_provider_router(spawn_stub_provider().await).await;

    // A model the provider does not list is rejected locally as model_not_found.
    let req = Request::builder()
        .uri("/v1/chat/completions")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(chat_request("composync/does-not-exist-model")))
        .unwrap();
    let resp = router.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(value["error"]["type"], serde_json::json!("model_not_found"));
    assert_eq!(
        value["error"]["message"],
        serde_json::json!(
            "Provider 'composync' does not offer model 'composync/does-not-exist-model'"
        )
    );

    // A listed model still reaches the provider.
    let req = Request::builder()
        .uri("/v1/chat/completions")
        .method("POST")
        .header("content-type", "application/json")
        .body(Body::from(chat_request("composync/known-model")))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
