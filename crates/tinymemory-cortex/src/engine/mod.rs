//! [`CortexEngine`]: TinyMemory's recall, fetch, store, list and forget over
//! CortexDB's event log.
//!
//! Each operation lives in its own module:
//!
//! - `store` — replay detection by item label, then one experience (or one
//!   ordered batch of turns), then the readability wait;
//! - `list` — a cursor over the kind scopes' listings, each item once;
//! - `fetch` — hybrid retrieval through recall packs, ranked by the engine;
//! - `recall` — one pack, one answer, citations from the pack;
//! - `forget` — look the items' events up, remove them by `memory_ids`.

mod cursor;
mod fetch;
mod forget;
mod items;
mod list;
mod recall;
mod store;

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tinymemory_api::{
    EngineDescriptor, EngineHealth, FetchPage, FetchRequest, ForgetReport, ForgetTarget, ListPage,
    ListRequest, MemoryEngine, RecallAnswer, RecallRequest, StoreItem, StoreReceipt,
};

use crate::credential::{BearerSource, CortexCredential};
use crate::descriptor::{CortexWire, Route};
use crate::error::{Error, Result};
use crate::log::Log;
use crate::transport::{HttpClient, health_reason, urlencode};

/// The scope prefix the hosted health probe lists under. The memory API
/// refuses a prefix that is not `type:id` segments (a bare word is a 400, which
/// would report a healthy service as broken); `tmh` is a type this crate never
/// writes, so the listing is empty and cheap.
const HEALTH_PROBE_SCOPE: &str = "tmh:probe";

/// The CortexDB memory engine, on either wire.
///
/// Build it with [`CortexEngine::direct`] for CortexDB's own API or
/// [`CortexEngine::tinyhumans`] for CortexDB behind the TinyHumans backend.
/// `Debug` shows the wire and endpoint origin, never the credential.
#[derive(Clone)]
pub struct CortexEngine {
    descriptor: EngineDescriptor,
    log: Log,
}

impl std::fmt::Debug for CortexEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CortexEngine")
            .field("id", &self.descriptor.id)
            .field("endpoint", &self.log.client.origin())
            .finish_non_exhaustive()
    }
}

impl CortexEngine {
    /// An engine on `wire` at `endpoint`, authenticating with `credential`.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] for an invalid or non-HTTP(S) endpoint, a cleartext
    /// endpoint that is not loopback (the credential would cross the network
    /// in the clear), or a blank static credential.
    pub fn new(wire: CortexWire, endpoint: &str, credential: CortexCredential) -> Result<Self> {
        Ok(Self {
            descriptor: wire.descriptor(),
            log: Log::new(HttpClient::new(wire, endpoint, credential)?),
        })
    }

    /// CortexDB's own `/v1/*` API at `endpoint` (for example
    /// [`crate::CORTEX_API_ENDPOINT`]), registered as `cortexdb`.
    ///
    /// # Errors
    ///
    /// As [`CortexEngine::new`].
    pub fn direct(endpoint: &str, credential: CortexCredential) -> Result<Self> {
        Self::new(CortexWire::Direct, endpoint, credential)
    }

    /// CortexDB behind the TinyHumans backend at `base_url` (for example
    /// [`crate::TINYHUMANS_API_ENDPOINT`]), registered as `tinyhumans`.
    /// `bearer` supplies the session JWT or `tiny_live_` API key and is
    /// consulted on every request, so a refreshed session is used at once.
    ///
    /// # Errors
    ///
    /// As [`CortexEngine::new`].
    pub fn tinyhumans(base_url: &str, bearer: Arc<dyn BearerSource>) -> Result<Self> {
        Self::new(
            CortexWire::TinyHumans,
            base_url,
            CortexCredential::Dynamic(bearer),
        )
    }

    /// Rebuilds the transport with a different per-request deadline (60s by
    /// default). A retrying read can take about three times this.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] if the HTTP client cannot be rebuilt.
    pub fn with_request_timeout(mut self, timeout: Duration) -> Result<Self> {
        self.log.client.set_timeout(timeout)?;
        Ok(self)
    }

    /// Which HTTP surface this engine talks to.
    #[must_use]
    pub fn wire(&self) -> CortexWire {
        self.log.client.wire()
    }
}

#[async_trait]
impl MemoryEngine for CortexEngine {
    fn descriptor(&self) -> &EngineDescriptor {
        &self.descriptor
    }

    /// Direct probes `v1/admin/health`; hosted lists one scope under a
    /// prefix this crate never writes (the backend has no health route, and
    /// this proves reachability and the credential in one round trip). An
    /// [`Error::Unavailable`] failure is `Degraded`, any other `Down`; the
    /// reason never carries the backend's own text.
    async fn health(&self) -> EngineHealth {
        let wire = self.wire();
        let path = match wire {
            CortexWire::Direct => wire.path(Route::Health).to_string(),
            CortexWire::TinyHumans => format!(
                "{}?prefix={}&limit=1",
                wire.path(Route::Health),
                urlencode(HEALTH_PROBE_SCOPE)
            ),
        };
        match self.log.client.probe(&path).await {
            Ok(()) => EngineHealth::Ok,
            Err(error @ Error::Unavailable(_)) => EngineHealth::Degraded(health_reason(&error)),
            Err(error) => EngineHealth::Down(health_reason(&error)),
        }
    }

    async fn recall(&self, req: RecallRequest) -> Result<RecallAnswer> {
        self.recall_answer(req).await
    }

    async fn fetch(&self, req: FetchRequest) -> Result<FetchPage> {
        self.fetch_page(req).await
    }

    async fn store(&self, item: StoreItem) -> Result<StoreReceipt> {
        self.store_item(item).await
    }

    async fn forget(&self, target: ForgetTarget) -> Result<ForgetReport> {
        self.forget_items(target).await
    }

    async fn list(&self, req: ListRequest) -> Result<ListPage> {
        self.list_page(req).await
    }

    /// By the items' id labels, one lookup per kind, rather than a scan.
    async fn get(&self, req: GetRequest) -> Result<Vec<Hit>> {
        self.get_items(req).await
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "mod_list_tests.rs"]
mod list_tests;

#[cfg(test)]
#[path = "mod_direct_tests.rs"]
mod direct_tests;

#[cfg(test)]
#[path = "mod_hosted_tests.rs"]
mod hosted_tests;

#[cfg(test)]
#[path = "engine_test_support.rs"]
mod test_support;
