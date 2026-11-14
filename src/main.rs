mod chunker;
mod db;
mod embedding;
mod extractor;
mod reranker;

use anyhow::{Context, Result, bail};
use avrig_logging::LoggingConfig;
use axum::{
    Router,
    extract::{DefaultBodyLimit, Multipart, Path, State},
    http::{HeaderMap, StatusCode, header},
    response::IntoResponse,
    routing::{get, head, post},
};
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use clap::{Parser, Subcommand};
use jsonwebtoken::{
    Algorithm, DecodingKey, Validation, decode, decode_header,
    jwk::{Jwk, JwkSet},
};
use notify::{Event, EventKind, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    env, fs,
    path::{Path as FsPath, PathBuf},
    sync::{Arc, RwLock},
    time::Duration,
};
use tokio::{
    fs::File,
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
    sync::mpsc,
};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

#[derive(Parser)]
#[command(
    name = "avrig",
    version,
    about = "Unified cross-platform avrig runtime"
)]
struct Cli {
    #[arg(short, long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    Run,
    Serve,
    Watch,
    Rerank,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Config {
    #[serde(default)]
    logging: LoggingSettings,
    #[serde(default)]
    node: NodeConfig,
    #[serde(default)]
    server: ServerConfig,
    #[serde(default)]
    watcher: WatcherConfig,
    #[serde(default)]
    reranker: reranker::RerankerConfig,
    #[serde(default)]
    oidc: OidcConfig,
    #[serde(default)]
    embedding: embedding::EmbeddingConfig,
    #[serde(default)]
    database: db::DatabaseConfig,
}

#[derive(Clone, Debug, Deserialize)]
struct LoggingSettings {
    #[serde(default = "default_log_level")]
    level: String,
    #[serde(default = "default_log_directory")]
    directory: PathBuf,
    #[serde(default = "default_log_size_mb")]
    max_size_mb: u64,
    #[serde(default = "default_log_archive_files")]
    max_archive_files: usize,
}

impl Default for LoggingSettings {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            directory: default_log_directory(),
            max_size_mb: default_log_size_mb(),
            max_archive_files: default_log_archive_files(),
        }
    }
}

fn default_log_level() -> String {
    "info".into()
}
fn default_log_directory() -> PathBuf {
    PathBuf::from("logs")
}
fn default_log_size_mb() -> u64 {
    10
}
fn default_log_archive_files() -> usize {
    5
}

#[derive(Clone, Debug, Default, Deserialize)]
struct NodeConfig {
    #[serde(default = "default_roles")]
    roles: Vec<String>,
}

fn default_roles() -> Vec<String> {
    vec!["server".into()]
}

#[derive(Clone, Debug, Deserialize)]
struct ServerConfig {
    #[serde(default = "default_host")]
    host: String,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default = "default_upload_dir")]
    upload_dir: PathBuf,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: default_host(),
            port: default_port(),
            upload_dir: default_upload_dir(),
        }
    }
}

fn default_host() -> String {
    "127.0.0.1".into()
}
fn default_port() -> u16 {
    8000
}
fn default_upload_dir() -> PathBuf {
    PathBuf::from("data/uploads")
}

#[derive(Clone, Debug, Default, Deserialize)]
struct WatcherConfig {
    #[serde(default)]
    targets: Vec<WatchTarget>,
    #[serde(default = "default_debounce")]
    debounce_ms: u64,
    #[serde(default)]
    poll: bool,
    #[serde(default)]
    api_url: Option<String>,
}

fn default_debounce() -> u64 {
    1000
}

#[derive(Clone, Debug, Deserialize)]
struct WatchTarget {
    path: PathBuf,
    #[serde(default)]
    project_id: Option<String>,
    #[serde(default)]
    knowledge_category: Option<String>,
    #[serde(default)]
    include_patterns: Vec<String>,
    #[serde(default)]
    exclude_patterns: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct OidcConfig {
    #[serde(default = "default_oidc_issuer")]
    issuer_url: String,
    #[serde(default = "default_oidc_discovery_url")]
    discovery_url: String,
    #[serde(default = "default_oidc_audience")]
    audience: String,
    #[serde(default = "default_required_scopes")]
    required_scopes: Vec<String>,
    #[serde(default = "default_jwks_refresh_seconds")]
    jwks_refresh_interval_seconds: u64,
    #[serde(default = "default_clock_skew_seconds")]
    clock_skew_seconds: u64,
    #[serde(default)]
    token_endpoint: Option<String>,
    #[serde(default)]
    client_id: Option<String>,
    #[serde(default)]
    client_secret: Option<String>,
}

impl Default for OidcConfig {
    fn default() -> Self {
        Self {
            issuer_url: default_oidc_issuer(),
            discovery_url: default_oidc_discovery_url(),
            audience: default_oidc_audience(),
            required_scopes: default_required_scopes(),
            jwks_refresh_interval_seconds: default_jwks_refresh_seconds(),
            clock_skew_seconds: default_clock_skew_seconds(),
            token_endpoint: None,
            client_id: None,
            client_secret: None,
        }
    }
}

fn default_oidc_issuer() -> String {
    "https://iam.avrig.ai/auth/v1/".into()
}
fn default_oidc_discovery_url() -> String {
    "https://iam.avrig.ai/auth/v1/.well-known/openid-configuration".into()
}
fn default_oidc_audience() -> String {
    "avrig-api".into()
}
fn default_required_scopes() -> Vec<String> {
    vec!["documents:read".into()]
}
fn default_jwks_refresh_seconds() -> u64 {
    300
}
fn default_clock_skew_seconds() -> u64 {
    60
}

#[derive(Clone, Debug, Deserialize)]
struct SecretsConfig {
    vault: VaultConfig,
    #[serde(default)]
    secrets: HashMap<String, HashMap<String, String>>,
}

#[derive(Clone, Debug, Deserialize)]
struct VaultConfig {
    address: String,
    #[serde(default = "default_transit_mount")]
    transit_mount: String,
    #[serde(default)]
    transit_key: String,
    #[serde(default = "default_vault_agent_token_file")]
    token_file: PathBuf,
}

#[derive(Clone)]
struct WatcherApiClient {
    http: reqwest::Client,
    api_url: String,
    token_provider: OidcTokenProvider,
}

#[derive(Clone)]
struct OidcTokenProvider {
    http: reqwest::Client,
    token_endpoint: String,
    client_id: String,
    client_secret: String,
    cached: Arc<tokio::sync::Mutex<Option<CachedAccessToken>>>,
}

#[derive(Clone)]
struct CachedAccessToken {
    access_token: String,
    expires_at: tokio::time::Instant,
}

#[derive(Deserialize)]
struct OidcTokenResponse {
    access_token: String,
    #[serde(default = "default_token_expiry")]
    expires_in: u64,
}

fn default_token_expiry() -> u64 {
    300
}

#[derive(Deserialize)]
struct VaultTransitDecryptResponse {
    data: VaultTransitDecryptData,
}

#[derive(Deserialize)]
struct VaultTransitDecryptData {
    plaintext: String,
}

impl SecretsConfig {
    async fn decrypt_secret(&self, client: &reqwest::Client, ciphertext: &str) -> Result<String> {
        let token = fs::read_to_string(&self.vault.token_file).with_context(|| {
            format!(
                "reading Vault token file {}",
                self.vault.token_file.display()
            )
        })?;
        let token = token.trim();
        let url = format!(
            "{}/v1/{}/decrypt/{}",
            self.vault.address.trim_end_matches('/'),
            self.vault.transit_mount.trim_matches('/'),
            self.vault.transit_key.trim_matches('/')
        );
        let response: VaultTransitDecryptResponse = client
            .post(&url)
            .header("X-Vault-Token", token)
            .json(&serde_json::json!({ "ciphertext": ciphertext }))
            .send()
            .await
            .context("sending decryption request to Vault")?
            .error_for_status()
            .context("Vault decryption failed")?
            .json()
            .await
            .context("decoding Vault transit response")?;
        let decoded = BASE64
            .decode(response.data.plaintext.trim())
            .context("decoding base64 plaintext from Vault transit")?;
        String::from_utf8(decoded).context("Vault plaintext is not valid UTF-8")
    }
}

impl OidcTokenProvider {
    async fn get_access_token(&self) -> Result<String> {
        let mut lock = self.cached.lock().await;
        if let Some(cached) = &*lock {
            if cached.expires_at > tokio::time::Instant::now() + Duration::from_secs(30) {
                return Ok(cached.access_token.clone());
            }
        }

        let response: OidcTokenResponse = self
            .http
            .post(&self.token_endpoint)
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("scope", "documents:read"),
            ])
            .send()
            .await
            .context("requesting OIDC access token")?
            .error_for_status()
            .context("OIDC token endpoint returned error status")?
            .json()
            .await
            .context("parsing OIDC token response")?;

        let token = response.access_token;
        *lock = Some(CachedAccessToken {
            access_token: token.clone(),
            expires_at: tokio::time::Instant::now() + Duration::from_secs(response.expires_in),
        });
        Ok(token)
    }
}

impl WatcherApiClient {
    async fn check_document_hash(&self, hash: &str) -> Result<Option<String>> {
        let token = self.token_provider.get_access_token().await?;
        let url = format!(
            "{}/api/v1/documents/by-hash/{}",
            self.api_url.trim_end_matches('/'),
            hash
        );
        let response = self
            .http
            .head(&url)
            .bearer_auth(token)
            .send()
            .await
            .with_context(|| format!("calling document hash endpoint {url}"))?;

        if response.status() == StatusCode::OK {
            let doc_id = response
                .headers()
                .get("x-document-id")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            Ok(doc_id.or_else(|| Some(String::new())))
        } else if response.status() == StatusCode::NOT_FOUND {
            Ok(None)
        } else {
            bail!(
                "document hash check failed with HTTP status {}",
                response.status()
            );
        }
    }
}

fn default_transit_mount() -> String {
    "transit".into()
}
fn default_vault_agent_token_file() -> PathBuf {
    PathBuf::from("vault-agent-token")
}

#[derive(Clone)]
struct AppState {
    docs: Arc<RwLock<HashMap<String, Document>>>,
    source_refs: Arc<RwLock<Vec<SourceRef>>>,
    upload_dir: PathBuf,
    oidc: Option<OidcValidator>,
    db: Arc<db::DbRepository>,
    embedder: Arc<embedding::OllamaEmbeddingClient>,
    embedding_config: embedding::EmbeddingConfig,
}

#[derive(Clone)]
struct OidcValidator {
    client: reqwest::Client,
    issuer: String,
    audience: String,
    required_scopes: Vec<String>,
    jwks_uri: String,
    jwks: Arc<tokio::sync::RwLock<Vec<Jwk>>>,
    clock_skew_seconds: u64,
}

#[derive(Deserialize)]
struct OidcDiscovery {
    issuer: String,
    jwks_uri: String,
}

#[derive(Debug, Deserialize)]
struct AccessClaims {
    #[serde(default)]
    scope: Option<String>,
    #[serde(default, alias = "scp")]
    scopes: Vec<String>,
}

impl OidcValidator {
    async fn new(config: OidcConfig) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .context("building OIDC HTTP client")?;
        let discovery: OidcDiscovery = client
            .get(&config.discovery_url)
            .send()
            .await
            .context("fetching OIDC discovery document")?
            .error_for_status()
            .context("OIDC discovery request failed")?
            .json()
            .await
            .context("decoding OIDC discovery document")?;
        if discovery.issuer != config.issuer_url {
            bail!(
                "OIDC discovery issuer mismatch: configured '{}', discovered '{}'",
                config.issuer_url,
                discovery.issuer
            );
        }
        let validator = Self {
            client,
            issuer: config.issuer_url,
            audience: config.audience,
            required_scopes: config.required_scopes,
            jwks_uri: discovery.jwks_uri,
            jwks: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            clock_skew_seconds: config.clock_skew_seconds,
        };
        validator.refresh_jwks().await?;
        info!(
            issuer = %validator.issuer,
            audience = %validator.audience,
            jwks_uri = %validator.jwks_uri,
            "Rauthy OIDC verifier initialized"
        );
        Ok(validator)
    }

    async fn refresh_jwks(&self) -> Result<()> {
        let jwks: JwkSet = self
            .client
            .get(&self.jwks_uri)
            .send()
            .await
            .context("fetching OIDC signing keys")?
            .error_for_status()
            .context("OIDC JWKS request failed")?
            .json()
            .await
            .context("decoding OIDC JWKS")?;
        if jwks.keys.is_empty() {
            bail!("OIDC JWKS document contains no signing keys");
        }
        *self.jwks.write().await = jwks.keys;
        Ok(())
    }

    async fn authenticate(&self, token: &str) -> bool {
        let Ok(header) = decode_header(token) else {
            return false;
        };
        if !matches!(header.alg, Algorithm::RS256 | Algorithm::EdDSA) {
            return false;
        }
        let Some(kid) = header.kid.as_deref() else {
            return false;
        };

        let key = {
            let keys = self.jwks.read().await;
            keys.iter()
                .find(|key| key.common.key_id.as_deref() == Some(kid))
                .cloned()
        };
        let key = match key {
            Some(key) => key,
            None => {
                if self.refresh_jwks().await.is_err() {
                    return false;
                }
                let keys = self.jwks.read().await;
                let Some(key) = keys
                    .iter()
                    .find(|key| key.common.key_id.as_deref() == Some(kid))
                    .cloned()
                else {
                    return false;
                };
                key
            }
        };
        let Ok(decoding_key) = DecodingKey::from_jwk(&key) else {
            return false;
        };
        let mut validation = Validation::new(header.alg);
        validation.algorithms = vec![Algorithm::RS256, Algorithm::EdDSA];
        validation.set_issuer(&[self.issuer.as_str()]);
        validation.set_audience(&[self.audience.as_str()]);
        validation.leeway = self.clock_skew_seconds;
        validation.required_spec_claims = ["exp", "iss", "aud"]
            .into_iter()
            .map(str::to_owned)
            .collect();

        let Ok(data) = decode::<AccessClaims>(token, &decoding_key, &validation) else {
            return false;
        };
        has_required_scopes(&data.claims, &self.required_scopes)
    }
}

fn has_required_scopes(claims: &AccessClaims, required_scopes: &[String]) -> bool {
    let mut token_scopes = claims.scopes.clone();
    if let Some(scope) = &claims.scope {
        token_scopes.extend(scope.split_ascii_whitespace().map(str::to_owned));
    }
    required_scopes
        .iter()
        .all(|required| token_scopes.iter().any(|scope| scope == required))
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    (scheme.eq_ignore_ascii_case("Bearer") && !token.trim().is_empty()).then_some(token.trim())
}

fn unauthorized() -> axum::response::Response {
    (
        StatusCode::UNAUTHORIZED,
        [(header::WWW_AUTHENTICATE, "Bearer")],
        "valid bearer token required",
    )
        .into_response()
}

#[derive(Clone, Debug, serde::Serialize)]
struct Document {
    id: String,
    sha256: String,
    file_name: String,
    mime_type: String,
    size_bytes: u64,
    storage_key: String,
    knowledge_category: Option<String>,
}

#[derive(Clone, Debug, serde::Serialize)]
struct SourceRef {
    document_id: String,
    source_path: String,
    source_platform: String,
    project_id: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let (config, secrets) = load_config(cli.config.as_deref())?;
    let log_settings = &config.logging;
    avrig_logging::init(LoggingConfig {
        directory: log_settings.directory.clone(),
        max_file_size_bytes: log_settings.max_size_mb.saturating_mul(1024 * 1024),
        max_archive_files: log_settings.max_archive_files,
        filter: log_settings.level.clone(),
    })
    .context("initializing avrig logging")?;
    let config = Arc::new(config);
    let roles = match cli.command {
        Some(Command::Run) | None => config.node.roles.clone(),
        Some(Command::Serve) => vec!["server".into()],
        Some(Command::Watch) => vec!["watcher".into()],
        Some(Command::Rerank) => vec!["reranker".into()],
    };
    for role in &roles {
        if role != "server" && role != "watcher" && role != "reranker" {
            bail!("unknown role '{role}', expected server, watcher, or reranker");
        }
    }
    if roles.is_empty() {
        bail!("no active roles configured");
    }

    // Load ciphertext configuration and validate the Vault agent flow, but do not
    // silently decrypt or print secrets until a real service requires them.
    if let Some(secrets) = &secrets {
        info!(
            vault = %secrets.vault.address,
            transit_mount = %secrets.vault.transit_mount,
            transit_key = %secrets.vault.transit_key,
            token_file = %secrets.vault.token_file.display(),
            secret_groups = secrets.secrets.len(),
            "Vault Transit configuration loaded"
        );
    }
    let db_repo = Arc::new(db::DbRepository::new(&config.database.url));
    let embed_client = Arc::new(embedding::OllamaEmbeddingClient::new(&config.embedding));

    let state = AppState {
        docs: Arc::new(RwLock::new(HashMap::new())),
        source_refs: Arc::new(RwLock::new(Vec::new())),
        upload_dir: config.server.upload_dir.clone(),
        oidc: None,
        db: db_repo,
        embedder: embed_client,
        embedding_config: config.embedding.clone(),
    };
    fs::create_dir_all(&state.upload_dir)
        .with_context(|| format!("creating upload directory {}", state.upload_dir.display()))?;

    let cancellation = CancellationToken::new();
    let signal_token = cancellation.clone();
    tokio::spawn(async move {
        if let Err(err) = wait_for_shutdown_signal().await {
            warn!(%err, "failed to listen for Ctrl+C");
        }
        signal_token.cancel();
    });

    let mut tasks = Vec::new();
    if roles.iter().any(|r| r == "server") {
        let cfg = config.clone();
        let task_state = state.clone();
        let token = cancellation.child_token();
        tasks.push(tokio::spawn(async move {
            run_server(cfg.server.clone(), cfg.oidc.clone(), task_state, token).await
        }));
    }
    if roles.iter().any(|r| r == "watcher") {
        let cfg = config.clone();
        let sec = secrets.clone();
        let token = cancellation.child_token();
        tasks.push(tokio::spawn(async move {
            run_watcher(cfg.watcher.clone(), cfg.oidc.clone(), sec, token).await
        }));
    }
    if roles.iter().any(|r| r == "reranker") {
        let cfg = config.clone();
        let token = cancellation.child_token();
        tasks.push(tokio::spawn(async move {
            run_reranker(cfg.reranker.clone(), token).await
        }));
    }

    tokio::select! {
        _ = cancellation.cancelled() => info!("shutdown requested"),
        result = async {
            for task in tasks {
                task.await.context("runtime task panicked")??;
            }
            Ok::<(), anyhow::Error>(())
        } => result?,
    }
    cancellation.cancel();
    Ok(())
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() -> std::io::Result<()> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    tokio::select! {
        result = tokio::signal::ctrl_c() => result,
        _ = terminate.recv() => Ok(()),
    }
}

#[cfg(windows)]
async fn wait_for_shutdown_signal() -> std::io::Result<()> {
    tokio::signal::ctrl_c().await
}

fn load_config(path: Option<&FsPath>) -> Result<(Config, Option<SecretsConfig>)> {
    let exe_dir = env::current_exe()
        .context("locating executable")?
        .parent()
        .context("executable has no parent directory")?
        .to_path_buf();
    let selected = path
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| exe_dir.join("config"));
    let files = if selected.is_file() {
        vec![selected]
    } else if selected.is_dir() {
        let mut paths = fs::read_dir(&selected)
            .with_context(|| format!("reading config directory {}", selected.display()))?
            .filter_map(|entry| entry.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|ext| ext == "toml"))
            .collect::<Vec<_>>();
        paths.sort();
        paths
    } else {
        bail!("configuration path does not exist: {}", selected.display());
    };

    let mut merged = toml::Value::Table(toml::map::Map::new());
    for file in &files {
        let contents = fs::read_to_string(file)
            .with_context(|| format!("reading config file {}", file.display()))?;
        let parsed: toml::Value = toml::from_str(&contents)
            .with_context(|| format!("parsing TOML file {}", file.display()))?;
        deep_merge(&mut merged, parsed);
    }
    let secrets: Option<SecretsConfig> = merged
        .get("vault")
        .cloned()
        .map(|vault| {
            let mut table = toml::map::Map::new();
            table.insert("vault".into(), vault);
            if let Some(values) = merged.get("secrets") {
                table.insert("secrets".into(), values.clone());
            }
            toml::Value::Table(table)
                .try_into()
                .context("parsing Vault secret configuration")
        })
        .transpose()?;
    let config: Config = merged.try_into().context("parsing runtime configuration")?;
    Ok((config, secrets))
}

fn deep_merge(base: &mut toml::Value, overlay: toml::Value) {
    match (base, overlay) {
        (toml::Value::Table(base), toml::Value::Table(overlay)) => {
            for (key, value) in overlay {
                match base.get_mut(&key) {
                    Some(existing) => deep_merge(existing, value),
                    None => {
                        base.insert(key, value);
                    }
                }
            }
        }
        (base, overlay) => *base = overlay,
    }
}

async fn run_server(
    config: ServerConfig,
    oidc_config: OidcConfig,
    mut state: AppState,
    cancellation: CancellationToken,
) -> Result<()> {
    let validator = OidcValidator::new(oidc_config.clone()).await?;
    state.oidc = Some(validator.clone());
    let refresh_validator = validator.clone();
    let refresh_token = cancellation.clone();
    let refresh_interval = Duration::from_secs(oidc_config.jwks_refresh_interval_seconds.max(30));
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(refresh_interval);
        interval.tick().await;
        loop {
            tokio::select! {
                _ = refresh_token.cancelled() => break,
                _ = interval.tick() => {
                    if let Err(err) = refresh_validator.refresh_jwks().await {
                        warn!(%err, "failed to refresh OIDC signing keys; retaining cached keys");
                    }
                }
            }
        }
    });
    let app = Router::new()
        .route("/api/v1/health", get(|| async { "ok" }))
        .route(
            "/api/v1/documents/by-hash/{sha256}",
            head(document_by_hash).get(document_by_hash),
        )
        .route("/api/v1/documents/upload", post(upload_document))
        .route("/api/v1/documents/search", post(search_documents))
        .route("/api/v1/documents/{id}/sources", get(document_sources))
        .with_state(state)
        .layer(DefaultBodyLimit::max(1024 * 1024 * 1024));

    let address = format!("{}:{}", config.host, config.port);
    let listener = TcpListener::bind(&address)
        .await
        .with_context(|| format!("binding server to {address}"))?;
    info!(%address, "HTTP API listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(cancellation.cancelled_owned())
        .await
        .context("HTTP server failed")
}

#[derive(Clone, Debug, Deserialize)]
struct RerankRequestPayload {
    query: String,
    documents: Vec<String>,
    #[serde(default)]
    top_k: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
struct RerankResponsePayload {
    query: String,
    count: usize,
    results: Vec<reranker::RerankItem>,
}

#[derive(Clone)]
struct RerankerState {
    reranker: Arc<reranker::NativeReranker>,
}

async fn run_reranker(
    config: reranker::RerankerConfig,
    cancellation: CancellationToken,
) -> Result<()> {
    info!(
        host = %config.host,
        port = config.port,
        model = %config.model,
        device = %config.device,
        "initializing native reranker service"
    );
    let native_reranker = reranker::NativeReranker::try_new(config.clone())
        .context("initializing NativeReranker")?;
    let state = RerankerState {
        reranker: Arc::new(native_reranker),
    };

    let app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/reranker", post(rerank_handler))
        .with_state(state)
        .layer(DefaultBodyLimit::max(64 * 1024 * 1024));

    let address = format!("{}:{}", config.host, config.port);
    let listener = TcpListener::bind(&address)
        .await
        .with_context(|| format!("binding reranker server to {address}"))?;
    info!(%address, "reranker HTTP API listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(cancellation.cancelled_owned())
        .await
        .context("reranker HTTP server failed")
}

async fn rerank_handler(
    State(state): State<RerankerState>,
    axum::Json(payload): axum::Json<RerankRequestPayload>,
) -> Result<axum::Json<RerankResponsePayload>, (StatusCode, String)> {
    let items = state
        .reranker
        .rerank(payload.query.clone(), payload.documents, payload.top_k)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("reranking error: {e}")))?;

    Ok(axum::Json(RerankResponsePayload {
        query: payload.query,
        count: items.len(),
        results: items,
    }))
}

async fn document_by_hash(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> impl IntoResponse {
    let Some(token) = bearer_token(&headers) else {
        return unauthorized();
    };
    let Some(oidc) = state.oidc.as_ref() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if !oidc.authenticate(token).await {
        return unauthorized();
    }
    if let Some(document) = state
        .docs
        .read()
        .expect("document lock poisoned")
        .get(&hash)
    {
        (StatusCode::OK, [("x-document-id", document.id.clone())]).into_response()
    } else {
        StatusCode::NOT_FOUND.into_response()
    }
}

async fn document_sources(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let refs = state.source_refs.read().expect("source lock poisoned");
    let matches = refs
        .iter()
        .filter(|source| source.document_id == id)
        .cloned()
        .collect::<Vec<_>>();
    axum::Json(matches).into_response()
}

async fn upload_document(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<axum::Json<serde_json::Value>, (StatusCode, String)> {
    // Authenticate if OIDC validator is configured
    if let Some(oidc) = state.oidc.as_ref() {
        let Some(token) = bearer_token(&headers) else {
            return Err((StatusCode::UNAUTHORIZED, "missing authorization bearer token".into()));
        };
        if !oidc.authenticate(token).await {
            return Err((StatusCode::UNAUTHORIZED, "invalid or expired token".into()));
        }
    }

    let mut file_name = String::new();
    let mut file_bytes = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" || field.file_name().is_some() {
            if let Some(fname) = field.file_name() {
                file_name = fname.to_string();
            }
            file_bytes = match field.bytes().await {
                Ok(b) => b.to_vec(),
                Err(e) => return Err((StatusCode::BAD_REQUEST, format!("reading multipart file data: {}", e))),
            };
            break;
        }
    }

    if file_bytes.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "no file uploaded or file empty".into()));
    }
    if file_name.is_empty() {
        file_name = "unnamed.bin".to_string();
    }

    // 1. Compute SHA-256
    let mut hasher = Sha256::new();
    hasher.update(&file_bytes);
    let sha256_hash = format!("{:x}", hasher.finalize());

    // 2. Check if already present
    if let Ok(Some(existing_id)) = state.db.check_document_by_hash(&sha256_hash).await {
        return Ok(axum::Json(serde_json::json!({
            "status": "already_indexed",
            "document_id": existing_id,
            "sha256": sha256_hash,
            "file_name": file_name,
        })));
    }

    // 3. Extract text
    let extracted = match extractor::extract_text(&file_name, &file_bytes) {
        Ok(ext) => ext,
        Err(e) => return Err((StatusCode::UNPROCESSABLE_ENTITY, format!("extracting document text: {}", e))),
    };

    // 4. Chunk text
    let chunks = chunker::chunk_document(
        &extracted,
        state.embedding_config.chunk_size,
        state.embedding_config.chunk_overlap,
    );

    if chunks.is_empty() {
        return Err((StatusCode::UNPROCESSABLE_ENTITY, "document produced 0 text chunks".into()));
    }

    // 5. Generate embeddings via Ollama
    let mut embeddings = Vec::new();
    for chunk in &chunks {
        match state.embedder.embed(&chunk.chunk_text).await {
            Ok(emb) => embeddings.push(emb),
            Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("generating embedding with Ollama: {}", e))),
        }
    }

    // 6. Insert document into km.document
    let doc_id = match state.db.insert_document(
        &file_name,
        &sha256_hash,
        &extracted.mime_type,
        file_bytes.len() as i64,
        &state.embedding_config.model,
    ).await {
        Ok(id) => id,
        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("inserting document record: {}", e))),
    };

    // 7. Insert chunk vectors into km.document_vector
    if let Err(e) = state.db.insert_document_vectors(doc_id, &chunks, &embeddings).await {
        return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("inserting document vectors: {}", e)));
    }

    info!(
        document_id = %doc_id,
        sha256 = %sha256_hash,
        file_name = %file_name,
        chunks = chunks.len(),
        "document uploaded, chunked, embedded, and indexed"
    );

    Ok(axum::Json(serde_json::json!({
        "status": "indexed",
        "document_id": doc_id,
        "sha256": sha256_hash,
        "file_name": file_name,
        "mime_type": extracted.mime_type,
        "chunks_indexed": chunks.len(),
        "embedding_model": state.embedding_config.model,
    })))
}

#[derive(Deserialize)]
struct SearchRequest {
    query: String,
    #[serde(default = "default_search_limit")]
    limit: i64,
    #[serde(default = "default_search_threshold")]
    threshold: f32,
}

fn default_search_limit() -> i64 { 5 }
fn default_search_threshold() -> f32 { 0.4 }

async fn search_documents(
    State(state): State<AppState>,
    headers: HeaderMap,
    axum::Json(payload): axum::Json<SearchRequest>,
) -> Result<axum::Json<serde_json::Value>, (StatusCode, String)> {
    if let Some(oidc) = state.oidc.as_ref() {
        let Some(token) = bearer_token(&headers) else {
            return Err((StatusCode::UNAUTHORIZED, "missing authorization bearer token".into()));
        };
        if !oidc.authenticate(token).await {
            return Err((StatusCode::UNAUTHORIZED, "invalid or expired token".into()));
        }
    }

    if payload.query.trim().is_empty() {
        return Err((StatusCode::BAD_REQUEST, "search query cannot be empty".into()));
    }

    // Embed query
    let query_embedding = match state.embedder.embed(&payload.query).await {
        Ok(emb) => emb,
        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("generating query embedding: {}", e))),
    };

    // Search vectors
    let results = match state.db.search_vectors(&query_embedding, payload.limit, payload.threshold).await {
        Ok(r) => r,
        Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("querying database vectors: {}", e))),
    };

    Ok(axum::Json(serde_json::json!({
        "query": payload.query,
        "count": results.len(),
        "results": results,
    })))
}

async fn run_watcher(
    config: WatcherConfig,
    oidc: OidcConfig,
    secrets: Option<SecretsConfig>,
    cancellation: CancellationToken,
) -> Result<()> {
    if config.targets.is_empty() {
        info!("watcher role active with no configured targets");
        cancellation.cancelled().await;
        return Ok(());
    }

    let http_client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .context("building watcher HTTP client")?;

    let api_client: Option<WatcherApiClient> = match (
        &config.api_url,
        &oidc.token_endpoint,
        &oidc.client_id,
    ) {
        (Some(api_url), Some(token_endpoint), Some(client_id)) => {
            let secret_ciphertext = oidc.client_secret.as_deref().or_else(|| {
                secrets
                    .as_ref()
                    .and_then(|s| s.secrets.get("watcher_oidc"))
                    .and_then(|g| g.get("client_secret_ciphertext"))
                    .map(String::as_str)
            });
            match secret_ciphertext {
                Some(ciphertext) => {
                    let decrypt_result = if let Some(sec) = secrets.as_ref() {
                        sec.decrypt_secret(&http_client, ciphertext).await
                    } else {
                        bail!("Vault configuration is required to decrypt client_secret");
                    };
                    match decrypt_result {
                        Ok(client_secret) => {
                            info!(api_url = %api_url, "watcher configured with authenticated API client");
                            Some(WatcherApiClient {
                                http: http_client.clone(),
                                api_url: api_url.clone(),
                                token_provider: OidcTokenProvider {
                                    http: http_client.clone(),
                                    token_endpoint: token_endpoint.clone(),
                                    client_id: client_id.clone(),
                                    client_secret,
                                    cached: Arc::new(tokio::sync::Mutex::new(None)),
                                },
                            })
                        }
                        Err(err) => {
                            warn!(%err, "failed to decrypt watcher OIDC client secret via Vault; remote hash check disabled");
                            None
                        }
                    }
                }
                None => {
                    warn!(
                        "watcher configured with api_url and oidc client credentials, but client_secret is missing"
                    );
                    None
                }
            }
        }
        (Some(api_url), _, _) => {
            warn!(%api_url, "watcher api_url configured without complete [oidc] token_endpoint and client_id");
            None
        }
        (None, _, _) => None,
    };
    let (tx, mut rx) = mpsc::channel::<notify::Result<Event>>(256);
    let mut watcher_handles = Vec::new();
    for target in &config.targets {
        if !target.path.exists() {
            bail!("watch target does not exist: {}", target.path.display());
        }
        let tx = tx.clone();
        if config.poll {
            let mut watcher = notify::PollWatcher::new(
                move |event| {
                    let _ = tx.blocking_send(event);
                },
                notify::Config::default()
                    .with_poll_interval(Duration::from_millis(config.debounce_ms.max(100))),
            )?;
            watcher.watch(&target.path, RecursiveMode::Recursive)?;
            let token = cancellation.clone();
            watcher_handles.push(tokio::task::spawn_blocking(move || {
                let _watcher = watcher;
                while !token.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }));
        } else {
            let mut watcher = notify::RecommendedWatcher::new(
                move |event| {
                    let _ = tx.blocking_send(event);
                },
                notify::Config::default(),
            )?;
            watcher.watch(&target.path, RecursiveMode::Recursive)?;
            let token = cancellation.clone();
            watcher_handles.push(tokio::task::spawn_blocking(move || {
                let _watcher = watcher;
                while !token.is_cancelled() {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }));
        }
        info!(path = %target.path.display(), "watching directory");
    }
    drop(tx);

    let mut last_seen: BTreeMap<PathBuf, tokio::time::Instant> = BTreeMap::new();
    let mut debounce_tick = tokio::time::interval(Duration::from_millis(100));
    loop {
        tokio::select! {
            _ = cancellation.cancelled() => break,
            _ = debounce_tick.tick() => {
                let ready = last_seen.iter()
                    .filter(|(_, at)| at.elapsed() >= Duration::from_millis(config.debounce_ms))
                    .map(|(path, _)| path.clone())
                    .collect::<Vec<_>>();
                for path in ready {
                    last_seen.remove(&path);
                    match sha256_file(&path).await {
                        Ok(hash) => {
                            let target = config.targets.iter().find(|target| path.starts_with(&target.path));
                            info!(
                                path = %path.display(),
                                %hash,
                                user_id = "system",
                                project_id = target.and_then(|t| t.project_id.as_deref()).unwrap_or("none"),
                                knowledge_category = target.and_then(|t| t.knowledge_category.as_deref()).unwrap_or("unspecified"),
                                "stable file ready for deduplication/upload"
                            );
                            if let Some(client) = &api_client {
                                match client.check_document_hash(&hash).await {
                                    Ok(Some(doc_id)) => {
                                        info!(
                                            %hash,
                                            document_id = %doc_id,
                                            "document already indexed on remote server; skipping binary upload"
                                        );
                                    }
                                    Ok(None) => {
                                        info!(
                                            %hash,
                                            "document not present on remote server; ready for upload"
                                        );
                                    }
                                    Err(err) => {
                                        warn!(%err, %hash, "remote document hash check query failed");
                                    }
                                }
                            } else if let Some(url) = &config.api_url {
                                warn!(%url, hash, "remote preflight/upload requires configured OIDC persistence client");
                            }
                        }
                        Err(err) => warn!(path = %path.display(), %err, "could not hash watched file"),
                    }
                }
            }
            event = rx.recv() => {
                let Some(event) = event else { break };
                match event {
                    Ok(event) if matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) => {
                        for path in event.paths {
                            if path.is_file() && watcher_accepts(&config, &path) {
                                last_seen.insert(path, tokio::time::Instant::now());
                            }
                        }
                    }
                    Ok(_) => {}
                    Err(err) => warn!(%err, "filesystem watcher error"),
                }
            }
        }
    }
    for handle in watcher_handles {
        let _ = handle.await;
    }
    Ok(())
}

fn watcher_accepts(config: &WatcherConfig, path: &FsPath) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let Some(extension) = path.extension().and_then(|ext| ext.to_str()) else {
        return false;
    };
    let extension = extension.to_ascii_lowercase();
    if !KNOWN_DOCUMENT_EXTENSIONS.contains(&extension.as_str()) {
        return false;
    }
    let target = config
        .targets
        .iter()
        .find(|target| path.starts_with(&target.path));
    let Some(target) = target else { return false };
    if target
        .exclude_patterns
        .iter()
        .any(|pattern| simple_match(pattern, name))
    {
        return false;
    }
    target.include_patterns.is_empty()
        || target
            .include_patterns
            .iter()
            .any(|pattern| simple_match(pattern, name))
}

const KNOWN_DOCUMENT_EXTENSIONS: &[&str] = &[
    "adoc", "csv", "doc", "docm", "docx", "epub", "htm", "html", "markdown", "md", "mdown", "mdx",
    "ods", "odt", "odp", "org", "pdf", "ppt", "pptm", "pptx", "rst", "rtf", "tex", "tsv", "txt",
    "xls", "xlsm", "xlsx", "xhtml",
];

fn simple_match(pattern: &str, name: &str) -> bool {
    if let Some(ext) = pattern.strip_prefix("*.") {
        return name
            .rsplit_once('.')
            .is_some_and(|(_, actual)| actual.eq_ignore_ascii_case(ext));
    }
    pattern == name || (pattern.starts_with('.') && name.starts_with(pattern))
}

async fn sha256_file(path: &FsPath) -> Result<String> {
    let mut file = File::open(path)
        .await
        .with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

async fn _write_upload(
    path: &FsPath,
    mut field: axum::extract::multipart::Field<'_>,
) -> Result<()> {
    let mut file = File::create(path).await?;
    while let Some(chunk) = field.chunk().await? {
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merges_nested_toml_tables_with_later_values_winning() {
        let mut base: toml::Value =
            toml::from_str("[server]\nport=8000\nhost='localhost'").unwrap();
        let overlay: toml::Value = toml::from_str("[server]\nport=9000").unwrap();
        deep_merge(&mut base, overlay);
        assert_eq!(base["server"]["port"].as_integer(), Some(9000));
        assert_eq!(base["server"]["host"].as_str(), Some("localhost"));
    }

    #[tokio::test]
    async fn hashes_files_incrementally() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sample.bin");
        fs::write(&path, b"avrig").unwrap();
        assert_eq!(
            sha256_file(&path).await.unwrap(),
            "8cc08c7ff25b0b2592e2e91dd521346ba2d50851db4981e54f77acb4ed1b979c"
        );
    }

    #[test]
    fn accepts_extension_patterns_and_honors_excludes() {
        let config = WatcherConfig {
            targets: vec![WatchTarget {
                path: PathBuf::from("docs"),
                project_id: None,
                knowledge_category: None,
                include_patterns: vec!["*.pdf".into(), "*.html".into()],
                exclude_patterns: vec!["*.tmp".into()],
            }],
            ..Default::default()
        };
        assert!(watcher_accepts(&config, FsPath::new("docs/file.pdf")));
        assert!(watcher_accepts(&config, FsPath::new("docs/page.html")));
        assert!(!watcher_accepts(&config, FsPath::new("docs/file.tmp")));
        assert!(!watcher_accepts(
            &config,
            FsPath::new("docs/workspace.json")
        ));
        assert!(!watcher_accepts(&config, FsPath::new("docs/image.png")));
    }

    #[test]
    fn loads_and_merges_config_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("01-base.toml"),
            "[node]\nroles=['server']\n[server]\nport=8000\n",
        )
        .unwrap();
        fs::write(dir.path().join("02-override.toml"), "[server]\nport=8123\n").unwrap();
        let (config, secrets) = load_config(Some(dir.path())).unwrap();
        assert_eq!(config.server.port, 8123);
        assert_eq!(config.node.roles, vec!["server"]);
        assert!(secrets.is_none());
    }

    #[test]
    fn bearer_header_is_required_and_scheme_is_case_insensitive() {
        let mut headers = HeaderMap::new();
        assert!(bearer_token(&headers).is_none());
        headers.insert(header::AUTHORIZATION, "Basic abc".parse().unwrap());
        assert!(bearer_token(&headers).is_none());
        headers.insert(
            header::AUTHORIZATION,
            "bEaReR signed.jwt.token".parse().unwrap(),
        );
        assert_eq!(bearer_token(&headers), Some("signed.jwt.token"));
    }

    #[test]
    fn required_document_scope_must_be_present() {
        let claims = AccessClaims {
            scope: Some("openid documents:read".into()),
            scopes: vec![],
        };
        assert!(has_required_scopes(
            &claims,
            &["documents:read".to_string()]
        ));
        assert!(!has_required_scopes(
            &claims,
            &["documents:write".to_string()]
        ));
    }

    #[tokio::test]
    async fn document_hash_check_rejects_requests_without_bearer_auth() {
        let emb_cfg = embedding::EmbeddingConfig::default();
        let state = AppState {
            docs: Arc::new(RwLock::new(HashMap::new())),
            source_refs: Arc::new(RwLock::new(Vec::new())),
            upload_dir: PathBuf::new(),
            oidc: None,
            db: Arc::new(db::DbRepository::new("postgres://localhost/test")),
            embedder: Arc::new(embedding::OllamaEmbeddingClient::new(&emb_cfg)),
            embedding_config: emb_cfg,
        };
        let response = document_by_hash(State(state), HeaderMap::new(), Path("a".repeat(64)))
            .await
            .into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer"
        );
    }

    #[tokio::test]
    async fn oidc_validator_rejects_hmac_signed_tokens() {
        let validator = OidcValidator {
            client: reqwest::Client::new(),
            issuer: "https://iam.avrig.ai/auth/v1/".into(),
            audience: "avrig-api".into(),
            required_scopes: vec!["documents:read".into()],
            jwks_uri: "https://iam.avrig.ai/auth/v1/oidc/certs".into(),
            jwks: Arc::new(tokio::sync::RwLock::new(Vec::new())),
            clock_skew_seconds: 60,
        };
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::new(Algorithm::HS256),
            &serde_json::json!({"exp": 4_000_000_000_u64}),
            &jsonwebtoken::EncodingKey::from_secret(b"not-an-oidc-key"),
        )
        .unwrap();
        assert!(!validator.authenticate(&token).await);
    }

    #[tokio::test]
    async fn watcher_api_client_handles_ok_and_not_found_responses() {
        let app = Router::new()
            .route(
                "/api/v1/documents/by-hash/{sha256}",
                head(|Path(hash): Path<String>| async move {
                    if hash == "known-hash" {
                        (StatusCode::OK, [("x-document-id", "doc-123")]).into_response()
                    } else {
                        StatusCode::NOT_FOUND.into_response()
                    }
                }),
            )
            .route(
                "/oauth/token",
                post(|| async {
                    axum::Json(serde_json::json!({
                        "access_token": "mock-token",
                        "expires_in": 3600
                    }))
                }),
            );

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let client = reqwest::Client::new();
        let watcher_client = WatcherApiClient {
            http: client.clone(),
            api_url: format!("http://{addr}"),
            token_provider: OidcTokenProvider {
                http: client,
                token_endpoint: format!("http://{addr}/oauth/token"),
                client_id: "test-client".into(),
                client_secret: "test-secret".into(),
                cached: Arc::new(tokio::sync::Mutex::new(None)),
            },
        };

        let found = watcher_client
            .check_document_hash("known-hash")
            .await
            .unwrap();
        assert_eq!(found.as_deref(), Some("doc-123"));

        let not_found = watcher_client
            .check_document_hash("unknown-hash")
            .await
            .unwrap();
        assert_eq!(not_found, None);
    }
}
