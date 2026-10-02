//! AgentMemory REST adapter.
//!
//! AgentMemory's public REST API models memories as free-form content rather
//! than records with user-defined metadata. This adapter therefore places a
//! versioned TinyMemory envelope in that content, while still using native
//! `remember` and `search` operations for persistence and recall.

use anyhow::Context;
use async_trait::async_trait;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tinymemory_api::recall::RecallOpts;
use tinymemory_api::traits::Memory;
use tinymemory_api::types::MemoryTaint;

use crate::common::{Attempts, Dialect, HttpClient, RemoteMemory, StoredEntry};

/// Stable driver id used by configuration and status output.
pub use tinymemory_api::drivers::AGENTMEMORY_DRIVER_ID;

/// Default URL of AgentMemory's local REST server.
pub const AGENTMEMORY_API_ENDPOINT: &str = "http://localhost:3111";

const ENVELOPE_KIND: &str = "tinymemory-agentmemory-v1";
const PAGE_SIZE: usize = 5_000;

/// An AgentMemory service exposed through TinyMemory's storage contract.
#[derive(Debug)]
pub struct AgentMemoryMemory {
    inner: RemoteMemory<AgentMemoryDialect>,
}

impl AgentMemoryMemory {
    /// Connects to an AgentMemory REST service.
    ///
    /// `secret` is sent as `Authorization: Bearer`; pass `None` for the local
    /// default, which has no API secret unless its operator configured one.
    ///
    /// # Errors
    ///
    /// Returns an error when `endpoint` is not an HTTP(S) URL.
    pub fn new(endpoint: &str, secret: Option<&str>) -> anyhow::Result<Self> {
        Ok(Self {
            inner: RemoteMemory::new(AgentMemoryDialect {
                client: HttpClient::bearer(endpoint, secret)?,
            }),
        })
    }

    /// Connects to AgentMemory running at [`AGENTMEMORY_API_ENDPOINT`].
    ///
    /// # Errors
    ///
    /// Returns an error when the default endpoint cannot be parsed.
    pub fn local(secret: Option<&str>) -> anyhow::Result<Self> {
        Self::new(AGENTMEMORY_API_ENDPOINT, secret)
    }

    /// Rebuilds the HTTP transport with a different per-request deadline.
    ///
    /// # Errors
    ///
    /// Fails only if the underlying HTTP client cannot be rebuilt.
    pub fn with_request_timeout(mut self, timeout: std::time::Duration) -> anyhow::Result<Self> {
        let client = self
            .inner
            .dialect_mut()
            .client
            .clone()
            .with_timeout(timeout)?;
        self.inner.dialect_mut().client = client;
        Ok(self)
    }
}

#[async_trait]
impl Memory for AgentMemoryMemory {
    fn name(&self) -> &str {
        self.inner.name()
    }
    async fn store(
        &self,
        n: &str,
        k: &str,
        c: &str,
        cat: tinymemory_api::types::MemoryCategory,
        s: Option<&str>,
    ) -> anyhow::Result<()> {
        self.inner.store(n, k, c, cat, s).await
    }
    async fn store_with_taint(
        &self,
        n: &str,
        k: &str,
        c: &str,
        cat: tinymemory_api::types::MemoryCategory,
        s: Option<&str>,
        t: MemoryTaint,
    ) -> anyhow::Result<()> {
        self.inner.store_with_taint(n, k, c, cat, s, t).await
    }
    async fn recall(
        &self,
        q: &str,
        l: usize,
        o: RecallOpts<'_>,
    ) -> anyhow::Result<Vec<tinymemory_api::types::MemoryEntry>> {
        self.inner.recall(q, l, o).await
    }
    async fn get(
        &self,
        n: &str,
        k: &str,
    ) -> anyhow::Result<Option<tinymemory_api::types::MemoryEntry>> {
        self.inner.get(n, k).await
    }
    async fn list(
        &self,
        n: Option<&str>,
        c: Option<&tinymemory_api::types::MemoryCategory>,
        s: Option<&str>,
    ) -> anyhow::Result<Vec<tinymemory_api::types::MemoryEntry>> {
        self.inner.list(n, c, s).await
    }
    async fn forget(&self, n: &str, k: &str) -> anyhow::Result<bool> {
        self.inner.forget(n, k).await
    }
    async fn namespace_summaries(
        &self,
    ) -> anyhow::Result<Vec<tinymemory_api::types::NamespaceSummary>> {
        self.inner.namespace_summaries().await
    }
    async fn count(&self) -> anyhow::Result<usize> {
        self.inner.count().await
    }
    async fn health_check(&self) -> bool {
        self.inner.health_check().await
    }
    async fn health_probe(&self) -> Option<tinymemory_api::health::MemoryHealth> {
        self.inner.health_probe().await
    }
}

#[derive(Debug)]
struct AgentMemoryDialect {
    client: HttpClient,
}

#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    adapter: String,
    entry: StoredEntry,
}

impl AgentMemoryDialect {
    fn encode(entry: StoredEntry) -> anyhow::Result<String> {
        Ok(serde_json::to_string(&Envelope {
            adapter: ENVELOPE_KIND.into(),
            entry,
        })?)
    }

    fn decode(value: &Value) -> Option<StoredEntry> {
        let id = value.get("id")?.as_str()?;
        let content = value.get("content")?.as_str()?;
        let mut envelope: Envelope = serde_json::from_str(content).ok()?;
        if envelope.adapter != ENVELOPE_KIND {
            return None;
        }
        envelope.entry.remote_id = id.to_owned();
        if envelope.entry.timestamp.is_empty() {
            envelope.entry.timestamp = value
                .get("updatedAt")
                .or_else(|| value.get("createdAt"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
        }
        Some(envelope.entry)
    }

    async fn memories(&self) -> anyhow::Result<Vec<Value>> {
        let mut offset = 0;
        let mut all = Vec::new();
        loop {
            let page: Value = self
                .client
                .json(
                    Method::GET,
                    &format!("agentmemory/memories?limit={PAGE_SIZE}&offset={offset}"),
                    None,
                    Attempts::RetryTransient,
                )
                .await?;
            let memories = page
                .get("memories")
                .and_then(Value::as_array)
                .context("AgentMemory memories response has no memories array")?;
            all.extend(memories.iter().cloned());
            if memories.len() < PAGE_SIZE {
                break;
            }
            offset += memories.len();
        }
        Ok(all)
    }

    async fn record(
        &self,
        namespace: &str,
        key: &str,
    ) -> anyhow::Result<Option<(String, StoredEntry)>> {
        Ok(self.memories().await?.into_iter().find_map(|memory| {
            let id = memory.get("id")?.as_str()?.to_owned();
            let entry = Self::decode(&memory)?;
            (entry.namespace == namespace && entry.key == key).then_some((id, entry))
        }))
    }
}

#[async_trait]
impl Dialect for AgentMemoryDialect {
    fn name(&self) -> &'static str {
        AGENTMEMORY_DRIVER_ID
    }

    async fn upsert(&self, entry: StoredEntry) -> anyhow::Result<()> {
        if let Some((id, _)) = self.record(&entry.namespace, &entry.key).await? {
            self.client
                .empty(
                    Method::POST,
                    "agentmemory/forget",
                    Some(&json!({"memoryId": id})),
                )
                .await?;
        }
        let content = Self::encode(entry)?;
        let _: Value = self
            .client
            .json(
                Method::POST,
                "agentmemory/remember",
                Some(&json!({"content": content, "type": "fact"})),
                Attempts::Once,
            )
            .await?;
        Ok(())
    }

    async fn entries(&self) -> anyhow::Result<Vec<StoredEntry>> {
        Ok(self
            .memories()
            .await?
            .iter()
            .filter_map(Self::decode)
            .collect())
    }

    async fn search(
        &self,
        query: &str,
        limit: usize,
        _opts: RecallOpts<'_>,
    ) -> anyhow::Result<Vec<StoredEntry>> {
        let response: Value = self
            .client
            .json(
                Method::POST,
                "agentmemory/search",
                Some(&json!({"query": query, "limit": limit})),
                Attempts::RetryTransient,
            )
            .await?;
        Ok(response
            .get("results")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|result| {
                let observation = result.get("observation")?;
                let content = observation
                    .get("narrative")
                    .or_else(|| observation.get("content"))?
                    .as_str()?;
                let mut envelope: Envelope = serde_json::from_str(content).ok()?;
                if envelope.adapter != ENVELOPE_KIND {
                    return None;
                }
                envelope.entry.remote_id = observation
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                envelope.entry.score = result.get("score").and_then(Value::as_f64);
                Some(envelope.entry)
            })
            .collect())
    }

    async fn delete(&self, namespace: &str, key: &str) -> anyhow::Result<bool> {
        let Some((id, _)) = self.record(namespace, key).await? else {
            return Ok(false);
        };
        self.client
            .empty(
                Method::POST,
                "agentmemory/forget",
                Some(&json!({"memoryId": id})),
            )
            .await?;
        Ok(true)
    }

    async fn health(&self) -> anyhow::Result<()> {
        self.client.probe("agentmemory/livez").await
    }
}

#[cfg(test)]
#[path = "agentmemory_tests.rs"]
mod test;
