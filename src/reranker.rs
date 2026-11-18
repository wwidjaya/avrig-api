use anyhow::{Context, Result, bail};
use fastembed::{RerankInitOptions, RerankResult, RerankerModel, TextRerank};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tracing::info;

/// Convert a raw reranker logit score to a percentage [0.0, 100.0] using standard sigmoid normalization.
pub fn logit_to_percentage(logit: f32) -> f32 {
    100.0 / (1.0 + (-logit).exp())
}

#[derive(Clone, Debug, Deserialize)]
pub struct RerankerConfig {
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Base URL of the reranker HTTP service as seen from the API gateway.
    ///
    /// This is the only source of the service address. The client appends
    /// `/reranker`. Leaving it unset (or empty) means no reranker is
    /// configured, and document search returns vector order without failing at
    /// startup. Deployment topology decides the concrete value, so no loopback
    /// host or port is assumed here.
    #[serde(default)]
    pub base_url: Option<String>,
    /// End-to-end timeout for a single `/reranker` call. A slow or stalled
    /// service degrades to vector ordering instead of blocking the request.
    #[serde(default = "default_request_timeout_ms")]
    pub request_timeout_ms: u64,
    /// Optional PEM bundle of additional root certificates used to verify the
    /// service certificate, for example the internal `avrig-ai` CA when the
    /// platform trust store does not already contain it.
    ///
    /// When absent, the native platform roots are used. Certificate
    /// verification is always on: this only adds trust anchors.
    #[serde(default)]
    pub ca_cert_path: Option<PathBuf>,
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_cache_dir")]
    pub cache_dir: PathBuf,
    #[serde(default = "default_device")]
    pub device: String,
    /// Result count the service keeps when a caller omits `top_k`.
    #[serde(default = "default_top_k")]
    pub default_top_k: usize,
    /// Upper bound on candidates per rerank call. Search uses this as the
    /// vector-retrieval depth, so it is also the largest candidate set the
    /// reranker sees for one query.
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
fn default_request_timeout_ms() -> u64 {
    60_000
}

impl Default for RerankerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            base_url: None,
            request_timeout_ms: default_request_timeout_ms(),
            ca_cert_path: None,
            model: default_model(),
            cache_dir: default_cache_dir(),
            device: default_device(),
            default_top_k: default_top_k(),
            max_candidates: default_max_candidates(),
        }
    }
}

impl RerankerConfig {
    /// Base URL used by the API gateway to reach the reranker service.
    ///
    /// `None` when no address is configured, which callers treat as "no
    /// reranker available" rather than an error.
    pub fn client_base_url(&self) -> Option<String> {
        self.base_url
            .as_deref()
            .map(str::trim)
            .filter(|url| !url.is_empty())
            .map(|url| url.trim_end_matches('/').to_string())
    }

    /// Fully resolved `POST` target on the reranker service.
    pub fn rerank_endpoint(&self) -> Option<String> {
        self.client_base_url()
            .map(|base| format!("{base}/reranker"))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RerankItem {
    pub index: usize,
    pub score: f32,
    pub document: String,
}

/// Score produced by the reranker for one candidate passage.
///
/// The value is the raw cross-encoder logit emitted by the model: fastembed
/// reads the `logits` tensor without a sigmoid, and the service applies no
/// softmax or normalisation across candidates, so a passage scores the same
/// alone or inside a larger candidate set. The scale is therefore unbounded and
/// is not a probability. Observed `bge-reranker-v2-m3` values include `+7.97`
/// and `-10.99`; the useful spread is roughly `-11.0..=8.0`, and the best
/// candidate in a weak result set is still negative.
///
/// A minimum score threshold of `0.0` is equivalent to sigmoid probability
/// `0.5`. Nothing in the contract enforces a range, so callers must treat the
/// value as an arbitrary finite `f32` and must not clamp it to `0.0..=1.0`.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
pub struct RerankScore {
    pub index: usize,
    pub score: f32,
}

#[derive(Debug, Error)]
pub enum RerankClientError {
    #[error("reranker request failed")]
    Transport(#[source] reqwest::Error),
    #[error("reranker returned HTTP status {status}")]
    UpstreamStatus { status: u16 },
    #[error("reranker returned an unusable response")]
    InvalidResponse,
}

impl RerankClientError {
    /// Stable, non-sensitive classifier for logging. The variants deliberately
    /// exclude upstream response bodies, request text, and URLs.
    pub fn safe_kind(&self) -> &'static str {
        match self {
            Self::Transport(err) if err.is_timeout() => "timeout",
            Self::Transport(_) => "transport",
            Self::UpstreamStatus { .. } => "upstream_status",
            Self::InvalidResponse => "invalid_response",
        }
    }
}

#[derive(Serialize)]
struct RerankerHttpRequest<'a> {
    query: &'a str,
    documents: &'a [String],
    top_k: Option<usize>,
}

#[derive(Deserialize)]
struct RerankerHttpResponse {
    query: String,
    count: usize,
    results: Vec<RerankerHttpResult>,
}

#[derive(Deserialize)]
struct RerankerHttpResult {
    index: usize,
    score: f32,
    document: String,
}

/// HTTP client for the reranker service route `POST /reranker`.
#[derive(Clone, Debug)]
pub struct RerankerClient {
    http: reqwest::Client,
    endpoint: String,
    default_top_k: usize,
    max_candidates: usize,
}

impl RerankerClient {
    /// Build the client when `[reranker] base_url` is configured.
    ///
    /// Returns `Ok(None)` when no address is set: the endpoint is a deployment
    /// concern, and a gateway without one must still serve searches. Errors are
    /// reserved for failing to construct the HTTP client itself.
    pub fn from_config(config: &RerankerConfig) -> Result<Option<Self>> {
        let Some(endpoint) = config.rerank_endpoint() else {
            info!("no reranker base_url configured; document search returns vector order only");
            return Ok(None);
        };
        let timeout = Duration::from_millis(config.request_timeout_ms.max(100));
        let mut builder = reqwest::Client::builder().timeout(timeout);
        let trust_roots = match config
            .ca_cert_path
            .as_deref()
            .filter(|path| !path.as_os_str().is_empty())
        {
            Some(path) => {
                let pem = std::fs::read(path)
                    .with_context(|| format!("reading reranker CA bundle {}", path.display()))?;
                let certificates = reqwest::Certificate::from_pem_bundle(&pem)
                    .with_context(|| format!("parsing reranker CA bundle {}", path.display()))?;
                if certificates.is_empty() {
                    bail!(
                        "reranker CA bundle {} contains no certificates",
                        path.display()
                    );
                }
                for certificate in certificates {
                    builder = builder.add_root_certificate(certificate);
                }
                path.display().to_string()
            }
            None => "platform".to_string(),
        };
        let http = builder.build().context("building reranker HTTP client")?;
        let client = Self {
            http,
            endpoint,
            default_top_k: config.default_top_k.max(1),
            max_candidates: config.max_candidates.max(1),
        };
        info!(
            endpoint = %client.endpoint,
            timeout_ms = config.request_timeout_ms,
            trust_roots = %trust_roots,
            default_top_k = client.default_top_k,
            max_candidates = client.max_candidates,
            "reranker client configured"
        );
        Ok(Some(client))
    }

    /// Candidate depth per search, bounded by the service's own limit.
    pub fn max_candidates(&self) -> usize {
        self.max_candidates
    }

    /// Rerank `documents` against `query` on the configured service.
    ///
    /// `top_k` is the number of scored candidates the service keeps; `None`
    /// falls back to `[reranker] default_top_k`. Search passes the full
    /// candidate count so a minimum-score filter applied after ranking still
    /// sees every scored passage.
    pub async fn rerank(
        &self,
        query: &str,
        documents: Vec<String>,
        top_k: Option<usize>,
    ) -> Result<Vec<RerankScore>, RerankClientError> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        let requested = documents.len();
        let payload = RerankerHttpRequest {
            query,
            documents: &documents,
            top_k: Some(top_k.unwrap_or(self.default_top_k)),
        };
        let response = self
            .http
            .post(&self.endpoint)
            .json(&payload)
            .send()
            .await
            .map_err(RerankClientError::Transport)?;
        let status = response.status();
        if !status.is_success() {
            // The service answers 422 with a plain-text body for malformed
            // requests and 5xx on inference failure. Neither body is surfaced.
            return Err(RerankClientError::UpstreamStatus {
                status: status.as_u16(),
            });
        }
        let decoded: RerankerHttpResponse = response
            .json()
            .await
            .map_err(|_| RerankClientError::InvalidResponse)?;
        if decoded.query != query
            || decoded.count != decoded.results.len()
            || decoded.results.len() > requested
            || decoded.results.iter().any(|item| {
                item.index >= requested
                    || !item.score.is_finite()
                    || item.document != documents[item.index]
            })
        {
            return Err(RerankClientError::InvalidResponse);
        }
        Ok(decoded
            .results
            .into_iter()
            .map(|item| RerankScore {
                index: item.index,
                score: item.score,
            })
            .collect())
    }
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
            init_options =
                init_options.with_execution_providers(vec![ort::ep::DirectML::default().build()]);
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
