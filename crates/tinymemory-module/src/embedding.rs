//! Embeddings stay host-side; only the compute request crosses.
//!
//! # The decision this module encodes
//!
//! The engine cannot recall anything without embedding a query, and embedding
//! means calling an inference provider that wants a credential. There were two
//! ways to give the module what it needs:
//!
//! 1. put the credential in the module's configuration, or
//! 2. keep the credential in the host and let the module ask the host to embed.
//!
//! This is the second. It is the same shape as the `tinywallet` module's
//! two-call signing split, and the reasoning transfers: a credential is not the
//! only thing that would have crossed with option 1. The host's provider
//! routing, its rate limiting, its cost accounting and its BYOK policy all hang
//! off the place where embedding happens, and moving embedding into the module
//! would have quietly moved all four out of the host's control — or, worse,
//! duplicated them.
//!
//! So [`BusEmbeddingHost::resolve_api_key`] returns `None`, unconditionally and
//! by construction. There is no configuration that makes it return a key,
//! because [`crate::config::ModuleConfig`] has no field to hold one.
//!
//! A loaded module shares this address space, so this is not a hard isolation
//! boundary and is not claimed as one — a hostile module could read the host's
//! keys out of process memory regardless. It is a refusal to widen a boundary
//! that already exists, which is worth doing on its own terms and costs one
//! in-process bus round trip per embed batch.
//!
//! # Why the synchronous getters carry data instead of calling
//!
//! `EmbeddingHost` is deliberately synchronous for everything except the embed
//! itself: `ollama_base_url`, `default_cloud_embedding_model` and
//! `model_supports_dimensions` are plain getters, called from deep inside
//! retrieval and sealing call stacks. They cannot `await` a bus call, so the
//! host passes their answers as configuration at load time. Only
//! [`EmbeddingProvider::embed`] is async, and it is the only method here that
//! touches the bus.

use std::sync::Arc;

use async_trait::async_trait;
use tinybus::Connection;
use tinymemory_api::host::{format_embedding_signature, EmbeddingHost, EmbeddingProvider};

use crate::config::ModuleConfig;

/// Well-known name the host serves its embedder under.
pub const EMBEDDING_HOST_BUS_NAME: &str = "ai.tinyhumans.tinymemory.EmbeddingHost";

/// Object path the host serves its embedder at.
pub const EMBEDDING_HOST_OBJECT_PATH: &str = "/ai/tinyhumans/tinymemory/EmbeddingHost";

/// Interface the host serves at [`EMBEDDING_HOST_OBJECT_PATH`].
///
/// Equal to [`EMBEDDING_HOST_BUS_NAME`] by convention, but a separate constant
/// because they are separate concepts to `TinyBus`: one addresses a peer, the
/// other selects a dispatch table on that peer's object.
pub const EMBEDDING_HOST_INTERFACE: &str = "ai.tinyhumans.tinymemory.EmbeddingHost";

/// The method the host exports.
const EMBED_METHOD: &str = "Embed";

/// Builds bus-backed providers, holding no credential of any kind.
pub struct BusEmbeddingHost {
    connection: Connection,
    ollama_base_url: String,
    cloud_model: String,
    cloud_dimensions: usize,
    models_supporting_dimensions: Vec<String>,
}

// `Connection` is not `Debug`, and the trait requires it. Rendering the
// connection would say nothing useful anyway.
impl std::fmt::Debug for BusEmbeddingHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BusEmbeddingHost")
            .field("cloud_model", &self.cloud_model)
            .field("cloud_dimensions", &self.cloud_dimensions)
            .finish_non_exhaustive()
    }
}

impl BusEmbeddingHost {
    /// Build a host bridge over `connection`, answering from `config`.
    #[must_use]
    pub fn new(connection: Connection, config: &ModuleConfig) -> Self {
        Self {
            connection,
            ollama_base_url: config.ollama_base_url.clone(),
            cloud_model: config.cloud_embedding_model.clone(),
            cloud_dimensions: config.cloud_embedding_dimensions,
            models_supporting_dimensions: config.models_supporting_dimensions.clone(),
        }
    }

    /// A provider that reports `name`/`model`/`dims` and embeds over the bus.
    fn provider(&self, name: &str, model: &str, dims: usize) -> BusEmbeddingProvider {
        BusEmbeddingProvider {
            connection: self.connection.clone(),
            name: name.to_string(),
            model_id: model.to_string(),
            dimensions: dims,
        }
    }
}

impl EmbeddingHost for BusEmbeddingHost {
    /// Always `None`. See the module docs: this module holds no credentials.
    ///
    /// `None` is not a degraded answer here. The trait documents it as "the
    /// provider has no stored credential", which is the literal truth for every
    /// provider from this module's point of view, and the keyed providers treat
    /// it as "authenticate some other way" — which the host does, on the far
    /// side of `Embed`.
    fn resolve_api_key(&self, _provider: &str) -> Option<String> {
        None
    }

    fn ollama_base_url(&self) -> String {
        self.ollama_base_url.clone()
    }

    /// The host's default is its managed-cloud embedder built from the same
    /// two values it sent as `cloud_embedding_model`/`cloud_embedding_dimensions`,
    /// so this provider is that embedder and names itself `cloud`: the name
    /// travels first on every `Embed` call and is what the host selects the
    /// credential and endpoint by. An invented label here (`module-bus`, once)
    /// reaches the host as an unknown provider slug and fails every batch.
    fn default_embedding_provider(&self) -> Arc<dyn EmbeddingProvider> {
        Arc::new(self.provider("cloud", &self.cloud_model.clone(), self.cloud_dimensions))
    }

    /// Builds a provider for an explicit triple, ignoring `api_key`.
    ///
    /// `api_key` is part of the trait's signature and is always empty here, both
    /// because `resolve_api_key` returns `None` and because the engine is handed
    /// an empty key at construction. It is ignored rather than rejected: a
    /// non-empty key would mean a caller inside this module had obtained one
    /// from somewhere, and failing the embed would be a worse outcome than
    /// simply not forwarding it.
    ///
    /// `custom_endpoint` is likewise not dialled. Whichever endpoint the host
    /// routes to is the host's decision, made on the far side of `Embed`.
    fn create_embedding_provider_with_credentials(
        &self,
        provider: &str,
        model: &str,
        dims: usize,
        _api_key: &str,
        _custom_endpoint: Option<&str>,
    ) -> Result<Box<dyn EmbeddingProvider>, String> {
        Ok(Box::new(self.provider(provider, model, dims)))
    }

    fn model_supports_dimensions(&self, model: &str) -> bool {
        self.models_supporting_dimensions
            .iter()
            .any(|known| known == model)
    }

    fn cloud_embedding_provider(
        &self,
        model: &str,
        dims: usize,
    ) -> Result<Box<dyn EmbeddingProvider>, String> {
        Ok(Box::new(self.provider("cloud", model, dims)))
    }

    fn default_cloud_embedding_model(&self) -> &str {
        &self.cloud_model
    }

    fn default_cloud_embedding_dimensions(&self) -> usize {
        self.cloud_dimensions
    }

    fn ollama_embedding_provider(
        &self,
        _base_url: &str,
        model: &str,
        dims: usize,
    ) -> Result<Box<dyn EmbeddingProvider>, String> {
        Ok(Box::new(self.provider("ollama", model, dims)))
    }
}

/// An [`EmbeddingProvider`] that asks the host to do the work.
pub struct BusEmbeddingProvider {
    connection: Connection,
    name: String,
    model_id: String,
    dimensions: usize,
}

impl std::fmt::Debug for BusEmbeddingProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BusEmbeddingProvider")
            .field("name", &self.name)
            .field("model_id", &self.model_id)
            .field("dimensions", &self.dimensions)
            // Non-exhaustive on purpose: `connection` is deliberately omitted, so
            // `Debug` output can never become a place a transport detail leaks.
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl EmbeddingProvider for BusEmbeddingProvider {
    fn name(&self) -> &str {
        &self.name
    }

    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    /// Embed `texts` by calling the host.
    ///
    /// The model and dimensionality this provider was built for travel with the
    /// request, so the host embeds into the space the engine believes it is
    /// writing into. A host that silently substituted a different model would
    /// split the embedding space, which is why the returned width is checked
    /// against [`Self::dimensions`] before the vectors are handed back — a
    /// mismatch is a hard error rather than vectors written into the wrong
    /// space, where they would become unsearchable without a re-embed.
    ///
    /// A zero-dimension provider is the engine's "semantic search off" state and
    /// is exempt from that check: it is expected to return empty vectors.
    ///
    /// # Errors
    ///
    /// The bus failure verbatim, or a width mismatch. Never carries the input
    /// text: a memory chunk is user content and an error string is not a place
    /// for it.
    async fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let proxy = self
            .connection
            .proxy(
                EMBEDDING_HOST_BUS_NAME,
                EMBEDDING_HOST_OBJECT_PATH,
                EMBEDDING_HOST_INTERFACE,
            )
            .map_err(|error| anyhow::anyhow!("embedding host unreachable: {error}"))?;

        let owned: Vec<String> = texts.iter().map(|text| (*text).to_string()).collect();
        log::debug!(
            "[tinymemory:module] embed batch={} model={} dims={}",
            owned.len(),
            self.model_id,
            self.dimensions
        );

        // Four positional arguments, in the order the host's `EmbeddingHost`
        // interface declares them: `(provider, model, dimensions, texts)`. The
        // host needs the provider name first because it is what selects the
        // credential and endpoint (`cloud` is its managed embedder, `ollama`
        // its local daemon, anything else a BYO-key slug). Sending three
        // arguments shifted `dimensions` into `model` and every embed was
        // refused at decode with "invalid type: integer, expected a string" —
        // nothing ingested in module mode ever got a vector (openhuman#5820).
        let vectors: Vec<Vec<f32>> = proxy
            .call(
                EMBED_METHOD,
                (
                    self.name.clone(),
                    self.model_id.clone(),
                    self.dimensions,
                    owned,
                ),
            )
            .await
            .map_err(|error| anyhow::anyhow!("host embed failed: {error}"))?;

        if vectors.len() != texts.len() {
            anyhow::bail!(
                "host returned {} vectors for {} inputs",
                vectors.len(),
                texts.len()
            );
        }
        // Checked unconditionally, including for a zero-dimension provider. An
        // earlier revision skipped the check entirely when `dimensions == 0`,
        // which let a host answer a "semantic search off" request with real
        // 768-wide vectors and pass — precisely the split-embedding-space
        // failure this check exists to prevent, except that the engine would
        // additionally believe no vectors existed at all. Zero dimensions means
        // empty vectors, and this is what says so.
        if let Some(bad) = vectors
            .iter()
            .find(|vector| vector.len() != self.dimensions)
        {
            anyhow::bail!(
                "host returned a {}-dimension vector for a {}-dimension space",
                bad.len(),
                self.dimensions
            );
        }

        Ok(vectors)
    }
}

/// The signature the engine will see for a bus-backed provider.
///
/// Exposed so a host can compute the same string without instantiating a
/// provider; drift between the two silently splits one embedding space in half.
#[must_use]
pub fn bus_provider_signature(name: &str, model_id: &str, dims: usize) -> String {
    format_embedding_signature(name, model_id, dims)
}

#[cfg(test)]
#[path = "embedding_tests.rs"]
mod test;
