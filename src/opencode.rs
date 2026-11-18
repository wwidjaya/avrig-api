use reqwest::{Client, Response, StatusCode};
use serde::Deserialize;
use serde_json::Value;
use std::{env, time::Duration};
use thiserror::Error;

const DEFAULT_MODEL: &str = "big-pickle";
const DEFAULT_ENDPOINT: &str = "https://opencode.ai/inference/openai/v1/chat/completions";
const DEFAULT_TIMEOUT_SECS: u64 = 120;

#[derive(Clone, Debug, Deserialize)]
pub struct OpenCodeConfig {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    #[serde(default = "default_api_key_env")]
    pub api_key_env: String,
}

fn default_model() -> String {
    DEFAULT_MODEL.into()
}

fn default_endpoint() -> String {
    DEFAULT_ENDPOINT.into()
}

fn default_timeout_secs() -> u64 {
    DEFAULT_TIMEOUT_SECS
}

fn default_api_key_env() -> String {
    "OPENCODE_CONSOLE_API_KEY".into()
}

impl Default for OpenCodeConfig {
    fn default() -> Self {
        Self {
            model: default_model(),
            endpoint: default_endpoint(),
            timeout_secs: default_timeout_secs(),
            api_key_env: default_api_key_env(),
        }
    }
}

#[derive(Debug, Error)]
pub enum OpenCodeError {
    #[error("OpenCode Console API key is not configured")]
    MissingApiKey,
    #[error("OpenCode Console request failed")]
    Request(#[from] reqwest::Error),
    #[error("OpenCode Console returned unsuccessful status {0}")]
    UpstreamStatus(StatusCode),
    #[error("chat completion request must be a JSON object")]
    InvalidRequest,
}

#[derive(Clone)]
pub struct OpenCodeClient {
    client: Client,
    endpoint: String,
    model: String,
    api_key: Option<String>,
}

impl OpenCodeClient {
    pub fn from_config(config: &OpenCodeConfig) -> Result<Self, OpenCodeError> {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()?;
        let api_key = env::var(&config.api_key_env)
            .ok()
            .filter(|key| !key.trim().is_empty());
        Ok(Self {
            client,
            endpoint: config.endpoint.clone(),
            model: config.model.clone(),
            api_key,
        })
    }

    pub async fn chat_completions(&self, mut payload: Value) -> Result<Response, OpenCodeError> {
        let api_key = self
            .api_key
            .as_deref()
            .ok_or(OpenCodeError::MissingApiKey)?;
        let request = payload
            .as_object_mut()
            .ok_or(OpenCodeError::InvalidRequest)?;
        request.insert("model".into(), Value::String(self.model.clone()));

        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(api_key)
            .json(&payload)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(OpenCodeError::UpstreamStatus(response.status()));
        }
        Ok(response)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Json, Router, routing::post};
    use std::sync::Arc;
    use tokio::{net::TcpListener, sync::Mutex};

    async fn serve(app: Router) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{address}/chat/completions")
    }

    #[tokio::test]
    async fn posts_openai_chat_payload_with_console_bearer_key_and_fixed_model() {
        let captured: Arc<Mutex<Option<(String, Value)>>> = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&captured);
        let app = Router::new().route(
            "/chat/completions",
            post(move |headers: axum::http::HeaderMap, Json(body): Json<Value>| {
                let sink = Arc::clone(&sink);
                async move {
                    *sink.lock().await = Some((
                        headers
                            .get("authorization")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_string(),
                        body,
                    ));
                    Json(serde_json::json!({
                        "id": "chatcmpl-test",
                        "object": "chat.completion",
                        "choices": [{
                            "index": 0,
                            "message": {"role": "assistant", "content": "hello from Big Pickle"},
                            "finish_reason": "stop"
                        }]
                    }))
                }
            }),
        );
        let endpoint = serve(app).await;
        let client = OpenCodeClient {
            client: Client::new(),
            endpoint,
            model: DEFAULT_MODEL.into(),
            api_key: Some("test-console-key".into()),
        };
        let response = client
            .chat_completions(serde_json::json!({
                "model": "caller-selected-model",
                "messages": [{"role": "user", "content": "say hello"}]
            }))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let (_, body) = captured.lock().await.clone().unwrap();
        assert_eq!(body["model"], DEFAULT_MODEL);
        assert_eq!(body["messages"][0]["content"], "say hello");
        let auth = captured.lock().await.as_ref().unwrap().0.clone();
        assert_eq!(auth, "Bearer test-console-key");
        let response: Value = response.json().await.unwrap();
        assert_eq!(
            response["choices"][0]["message"]["content"],
            "hello from Big Pickle"
        );
    }

    #[tokio::test]
    async fn rejects_missing_console_api_key_before_sending() {
        let client = OpenCodeClient {
            client: Client::new(),
            endpoint: "http://127.0.0.1:1/chat/completions".into(),
            model: DEFAULT_MODEL.into(),
            api_key: None,
        };
        let err = client
            .chat_completions(serde_json::json!({"messages": []}))
            .await
            .unwrap_err();
        assert!(matches!(err, OpenCodeError::MissingApiKey));
    }

    #[tokio::test]
    async fn surfaces_console_forbidden_status_for_model_access_errors() {
        let app = Router::new().route(
            "/chat/completions",
            post(|| async { StatusCode::FORBIDDEN }),
        );
        let client = OpenCodeClient {
            client: Client::new(),
            endpoint: serve(app).await,
            model: DEFAULT_MODEL.into(),
            api_key: Some("test-console-key".into()),
        };
        let err = client
            .chat_completions(serde_json::json!({"messages": []}))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            OpenCodeError::UpstreamStatus(StatusCode::FORBIDDEN)
        ));
    }
}
