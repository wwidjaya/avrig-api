use anyhow::{Context, Result, anyhow};
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct EmbeddingConfig {
    #[serde(default = "default_provider")]
    pub provider: String,
    #[serde(default = "default_base_url")]
    pub base_url: String,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_dimension")]
    pub dimension: usize,
    #[serde(default = "default_chunk_size")]
    pub chunk_size: usize,
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: usize,
}

fn default_provider() -> String {
    "ollama".into()
}
fn default_base_url() -> String {
    "http://172.26.0.1:11434".into()
}
fn default_model() -> String {
    "bge-m3:latest".into()
}
fn default_dimension() -> usize {
    1024
}
fn default_chunk_size() -> usize {
    512
}
fn default_chunk_overlap() -> usize {
    64
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: default_provider(),
            base_url: default_base_url(),
            model: default_model(),
            dimension: default_dimension(),
            chunk_size: default_chunk_size(),
            chunk_overlap: default_chunk_overlap(),
        }
    }
}

#[derive(Serialize)]
struct OllamaEmbedRequest<'a> {
    model: &'a str,
    prompt: &'a str,
}

#[derive(Deserialize)]
struct OllamaEmbedResponse {
    embedding: Vec<f32>,
}

#[derive(Clone)]
pub struct OllamaEmbeddingClient {
    client: Client,
    base_url: String,
    model: String,
    expected_dim: usize,
}

impl OllamaEmbeddingClient {
    pub fn new(config: &EmbeddingConfig) -> Self {
        Self {
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(60))
                .build()
                .unwrap(),
            base_url: config.base_url.trim_end_matches('/').to_string(),
            model: config.model.clone(),
            expected_dim: config.dimension,
        }
    }

    pub async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let url = format!("{}/api/embeddings", self.base_url);
        let req = OllamaEmbedRequest {
            model: &self.model,
            prompt: text,
        };

        let res = self
            .client
            .post(&url)
            .json(&req)
            .send()
            .await
            .with_context(|| format!("calling Ollama embeddings at {}", url))?;

        if !res.status().is_success() {
            let status = res.status();
            let err_body = res.text().await.unwrap_or_default();
            return Err(anyhow!("Ollama error {}: {}", status, err_body));
        }

        let parsed: OllamaEmbedResponse = res
            .json()
            .await
            .context("deserializing Ollama embedding response")?;

        if parsed.embedding.len() != self.expected_dim {
            return Err(anyhow!(
                "embedding dimension mismatch: expected {}, got {}",
                self.expected_dim,
                parsed.embedding.len()
            ));
        }

        Ok(parsed.embedding)
    }
}
