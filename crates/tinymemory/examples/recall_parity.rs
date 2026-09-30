//! Recall parity between the embedded engine and hosted `CortexDB`.
//!
//! Runs [`tinymemory::conformance::parity`]'s bundled corpus against the full
//! embedded `TinycortexProvider` (a fresh temporary workspace) and, when its
//! credentials are set, against `CortexDB` hosted by the `TinyHumans` backend,
//! then prints one markdown table: hit@1, hit@5, MRR, and store / recall
//! latency per engine.
//!
//! ```sh
//! # Hosted (optional): a scratch account's origin and bearer.
//! export TINYMEMORY_TEST_TINYHUMANS_URL=https://api.tinyhumans.ai
//! export TINYMEMORY_TEST_TINYHUMANS_TOKEN=...
//! # The embedded engine's embedder (optional; keyword-only recall without
//! # one). Any OpenAI-compatible `/embeddings` endpoint — for example the
//! # backend's managed one at `https://api.tinyhumans.ai/openai/v1`, which is
//! # what the desktop app embeds with by default.
//! export TINYMEMORY_PARITY_EMBED_URL=https://api.tinyhumans.ai/openai/v1
//! export TINYMEMORY_PARITY_EMBED_KEY=...          # defaults to the hosted token
//! export TINYMEMORY_PARITY_EMBED_MODEL=...        # the endpoint's model name
//! export TINYMEMORY_PARITY_EMBED_DIMS=1024
//! cargo run -p tinymemory --example recall_parity \
//!     --features tinycortex,core,tinyhumans,conformance
//! ```
//!
//! Hosted runs spend the account's credits and are bound by the backend's
//! 300-requests-a-minute limit; a run is about a hundred and fifty requests.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tinymemory::api::host::{
    EmbeddingHost, EmbeddingProvider, LocalAiConfig, MemoryConfig, MemoryTreeConfig, NoopEmbedding,
    SchedulerGateConfig,
};
use tinymemory::conformance::parity::{measure, ParityReport, BUNDLED_CORPUS};
use tinymemory::remote::{tinyhumans_provider, StaticBearer};
use tinymemory::tinycortex::engine::{EngineRuntimeConfig, TinycortexProvider};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let hosted = env("TINYMEMORY_TEST_TINYHUMANS_URL").zip(env("TINYMEMORY_TEST_TINYHUMANS_TOKEN"));
    let embedding = remote_embedder(hosted.as_ref().map(|(_, token)| token.as_str()));
    let embedded_label = match &embedding {
        Some(remote) => format!("embedded ({})", remote.model),
        None => "embedded (keyword only)".to_string(),
    };

    let mut rows = Vec::new();
    let workspace = scratch_workspace()?;
    let local = embedded_provider(&workspace, embedding)?;
    let report = measure(&local, &run_namespace(), &BUNDLED_CORPUS).await?;
    rows.push((embedded_label, report));
    let _ = std::fs::remove_dir_all(&workspace);

    if let Some((url, token)) = hosted {
        let provider = tinyhumans_provider(&url, Arc::new(StaticBearer::new(token)))?;
        let report = measure(&provider, &run_namespace(), &BUNDLED_CORPUS).await?;
        rows.push(("hosted CortexDB".to_string(), report));
    }

    println!("{}", ParityReport::markdown_header());
    for (label, report) in &rows {
        println!("{}", report.markdown_row(label));
    }
    for (label, report) in &rows {
        if !report.missed.is_empty() {
            println!("\n{label} missed: {}", report.missed.join(", "));
        }
    }
    Ok(())
}

/// A non-empty environment variable.
fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}

/// A namespace no other run shares.
fn run_namespace() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("tinymemory-parity/run{nanos}")
}

/// A fresh directory for the embedded engine's store.
fn scratch_workspace() -> anyhow::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("tinymemory-parity-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// The full embedded provider over `workspace`, embedding with `embedder` when
/// one is configured.
fn embedded_provider(
    workspace: &std::path::Path,
    embedder: Option<RemoteEmbedder>,
) -> anyhow::Result<TinycortexProvider> {
    let provider: Arc<dyn EmbeddingProvider> = match embedder {
        Some(embedder) => Arc::new(embedder),
        None => Arc::new(NoopEmbedding),
    };
    tinymemory::core::embedding_host::set_embedding_host(Arc::new(ParityHost { provider }));
    let client = Arc::new(
        tinymemory::core::store::MemoryClient::from_workspace_dir(workspace.to_path_buf())
            .map_err(anyhow::Error::msg)?,
    );
    let config = EngineRuntimeConfig {
        workspace_dir: workspace.to_path_buf(),
        config_path: workspace.join("config.toml"),
        memory: MemoryConfig::default(),
        memory_tree: MemoryTreeConfig::default(),
        scheduler_gate: SchedulerGateConfig::default(),
        local_ai: LocalAiConfig::default(),
        embeddings_provider: None,
        memory_provider: None,
        default_model: None,
        default_temperature: 0.2,
        output_language: None,
        memory_sources: serde_json::Value::Null,
        memory_sync_interval_secs: None,
        composio_mode: String::new(),
        backend_api_url: String::new(),
        composio_entity_id: String::new(),
    };
    Ok(TinycortexProvider::new("tinycortex".into(), config, client))
}

/// The embedding host the embedded engine asks for its embedder: one
/// provider, whatever it asks for.
struct ParityHost {
    provider: Arc<dyn EmbeddingProvider>,
}

impl std::fmt::Debug for ParityHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ParityHost")
            .field("model", &self.provider.model_id())
            .finish()
    }
}

impl ParityHost {
    fn boxed(&self) -> Box<dyn EmbeddingProvider> {
        Box::new(Shared(self.provider.clone()))
    }
}

impl EmbeddingHost for ParityHost {
    fn resolve_api_key(&self, _provider: &str) -> Option<String> {
        None
    }

    fn ollama_base_url(&self) -> String {
        "http://127.0.0.1:1".into()
    }

    fn default_embedding_provider(&self) -> Arc<dyn EmbeddingProvider> {
        self.provider.clone()
    }

    fn create_embedding_provider_with_credentials(
        &self,
        _provider: &str,
        _model: &str,
        _dims: usize,
        _api_key: &str,
        _custom_endpoint: Option<&str>,
    ) -> Result<Box<dyn EmbeddingProvider>, String> {
        Ok(self.boxed())
    }

    fn model_supports_dimensions(&self, _model: &str) -> bool {
        false
    }

    fn cloud_embedding_provider(
        &self,
        _model: &str,
        _dims: usize,
    ) -> Result<Box<dyn EmbeddingProvider>, String> {
        Ok(self.boxed())
    }

    fn default_cloud_embedding_model(&self) -> &'static str {
        "parity"
    }

    fn default_cloud_embedding_dimensions(&self) -> usize {
        self.provider.dimensions()
    }

    fn ollama_embedding_provider(
        &self,
        _base_url: &str,
        _model: &str,
        _dims: usize,
    ) -> Result<Box<dyn EmbeddingProvider>, String> {
        Ok(self.boxed())
    }
}

/// An `Arc`'d provider behind the `Box` some host methods return.
struct Shared(Arc<dyn EmbeddingProvider>);

#[async_trait]
impl EmbeddingProvider for Shared {
    fn name(&self) -> &str {
        self.0.name()
    }

    fn model_id(&self) -> &str {
        self.0.model_id()
    }

    fn dimensions(&self) -> usize {
        self.0.dimensions()
    }

    async fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        self.0.embed(texts).await
    }
}

/// How long one embeddings request may take, body included.
const EMBED_TIMEOUT: std::time::Duration = std::time::Duration::from_mins(1);

/// An OpenAI-compatible `/embeddings` endpoint, from the environment.
struct RemoteEmbedder {
    http: reqwest::Client,
    base: String,
    key: String,
    model: String,
    dims: usize,
}

/// The configured embedder, or `None` when no URL or model is set.
/// `fallback_key` (the hosted token) is used when no key of its own is set.
fn remote_embedder(fallback_key: Option<&str>) -> Option<RemoteEmbedder> {
    let base = env("TINYMEMORY_PARITY_EMBED_URL")?;
    let model = env("TINYMEMORY_PARITY_EMBED_MODEL")?;
    let key = env("TINYMEMORY_PARITY_EMBED_KEY").or_else(|| fallback_key.map(str::to_owned))?;
    let dims = env("TINYMEMORY_PARITY_EMBED_DIMS")
        .and_then(|dims| dims.parse().ok())
        .unwrap_or(1024);
    Some(RemoteEmbedder {
        http: reqwest::Client::new(),
        base: base.trim_end_matches('/').to_string(),
        key,
        model,
        dims,
    })
}

#[async_trait]
impl EmbeddingProvider for RemoteEmbedder {
    fn name(&self) -> &'static str {
        "parity-remote"
    }

    fn model_id(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dims
    }

    async fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        #[derive(serde::Deserialize)]
        struct Answer {
            data: Vec<Item>,
        }
        #[derive(serde::Deserialize)]
        struct Item {
            embedding: Vec<f32>,
        }
        let answer: Answer = self
            .http
            .post(format!("{}/embeddings", self.base))
            .bearer_auth(&self.key)
            .json(&serde_json::json!({ "model": self.model, "input": texts }))
            // From connecting to the end of the body, so an endpoint that
            // accepts and then stalls fails the run instead of hanging it.
            .timeout(EMBED_TIMEOUT)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(answer.data.into_iter().map(|item| item.embedding).collect())
    }
}
