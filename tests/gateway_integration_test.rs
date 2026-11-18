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
