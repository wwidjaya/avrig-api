use std::path::PathBuf;
use std::sync::Arc;
use anyhow::{Context, Result};
use fastembed::{RerankInitOptions, RerankResult, RerankerModel, TextRerank};
use serde::{Deserialize, Serialize};
use tracing::info;

#[derive(Clone, Debug, Deserialize)]
pub struct RerankerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_cache_dir")]
    pub cache_dir: PathBuf,
    #[serde(default = "default_device")]
    pub device: String,
    #[serde(default = "default_top_k")]
    pub default_top_k: usize,
    #[serde(default = "default_max_candidates")]
    pub max_candidates: usize,
}

fn default_host() -> String {
    "127.0.0.1".into()
}
fn default_port() -> u16 {
    8001
}
fn default_model() -> String {
    "bge-reranker-v2-m3".into()
}
fn default_cache_dir() -> PathBuf {
    PathBuf::from("data/models/cache")
}
fn default_device() -> String {
    "xpu".into()
}
fn default_top_k() -> usize {
    5
}
fn default_max_candidates() -> usize {
    100
}

impl Default for RerankerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            model: default_model(),
            cache_dir: default_cache_dir(),
            device: default_device(),
            default_top_k: default_top_k(),
            max_candidates: default_max_candidates(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RerankItem {
    pub index: usize,
    pub score: f32,
    pub document: String,
}

#[derive(Clone)]
pub struct NativeReranker {
    model: Arc<tokio::sync::Mutex<TextRerank>>,
    pub config: RerankerConfig,
}

impl NativeReranker {
    pub fn try_new(config: RerankerConfig) -> Result<Self> {
        let model_type = match config.model.as_str() {
            "bge-reranker-v2-m3" | "BGERerankerV2M3" => RerankerModel::BGERerankerV2M3,
            "bge-reranker-base" | "BGERerankerBase" => RerankerModel::BGERerankerBase,
            _ => {
                info!(model = %config.model, "defaulting to BGERerankerV2M3");
                RerankerModel::BGERerankerV2M3
            }
        };

        let mut init_options = RerankInitOptions::new(model_type)
            .with_cache_dir(config.cache_dir.clone())
            .with_show_download_progress(true);

        #[cfg(windows)]
        if config.device.eq_ignore_ascii_case("xpu")
            || config.device.eq_ignore_ascii_case("directml")
            || config.device.eq_ignore_ascii_case("gpu")
        {
            info!("initializing fastembed reranker with DirectML execution provider on Windows");
            init_options = init_options.with_execution_providers(vec![
                ort::ep::DirectML::default().build(),
            ]);
        }

        let model = TextRerank::try_new(init_options)
            .context("failed to initialize fastembed TextRerank model")?;

        info!(
            model = %config.model,
            device = %config.device,
            cache_dir = %config.cache_dir.display(),
            "Native fastembed reranker initialized successfully"
        );

        Ok(Self {
            model: Arc::new(tokio::sync::Mutex::new(model)),
            config,
        })
    }

    pub async fn rerank(
        &self,
        query: String,
        mut documents: Vec<String>,
        top_k: Option<usize>,
    ) -> Result<Vec<RerankItem>> {
        let limit = top_k.unwrap_or(self.config.default_top_k);
        if documents.len() > self.config.max_candidates {
            documents.truncate(self.config.max_candidates);
        }

        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let docs_for_task = documents.clone();
        let model_lock = Arc::clone(&self.model);

        // spawn_blocking handles batch inference without blocking the async executor
        let ranked: Vec<RerankResult> = tokio::task::spawn_blocking(move || {
            let mut guard = model_lock.blocking_lock();
            let query_str = query.as_str();
            let doc_refs: Vec<&str> = docs_for_task.iter().map(|s| s.as_str()).collect();
            guard.rerank::<&str>(query_str, doc_refs.as_slice(), true, None)
        })
        .await
        .context("rerank background task panicked")?
        .context("reranking execution failed")?;

        let mut items = Vec::with_capacity(ranked.len().min(limit));
        for item in ranked.into_iter().take(limit) {
            let doc = documents.get(item.index).cloned().unwrap_or_default();
            items.push(RerankItem {
                index: item.index,
                score: item.score,
                document: doc,
            });
        }

        Ok(items)
    }
}
