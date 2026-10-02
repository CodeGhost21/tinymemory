//! Self-hosted Cognee REST adapter.

use anyhow::Context;
use async_trait::async_trait;
use reqwest::{multipart, Method};
use serde_json::{json, Value};
use tinymemory_api::recall::RecallOpts;
use tinymemory_api::traits::Memory;
use tinymemory_api::types::MemoryTaint;

use crate::common::{stable_id, Attempts, Dialect, HttpClient, RemoteMemory, StoredEntry};

/// Stable driver id used by configuration and status output.
pub use tinymemory_api::drivers::COGNEE_DRIVER_ID;

/// A Cognee managed or self-hosted service exposed through TinyMemory's contract.
#[derive(Debug)]
pub struct CogneeMemory {
    inner: RemoteMemory<CogneeDialect>,
}

impl CogneeMemory {
    /// Rebuilds the HTTP transport with a different per-request deadline
    /// (issue #18 follow-up U5). The default is 60s with a 10s connect
    /// deadline — right for interactive calls; a bulk migration or a tight
    /// liveness probe may want its own budget.
    ///
    /// # Errors
    ///
    /// Fails only if the underlying HTTP client cannot be rebuilt — a
    /// configuration-time failure, before any request is made.
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

    /// Connect to a self-hosted Cognee server.
    ///
    /// `access_token` is sent as a bearer token. Local deployments with
    /// backend access control disabled may pass `None`.
    ///
    /// # Errors
    ///
    /// Returns an error when `endpoint` is not an HTTP(S) URL.
    pub fn new(endpoint: &str, access_token: Option<&str>) -> anyhow::Result<Self> {
        Self::self_hosted(endpoint, access_token)
    }

    /// Connect to a self-hosted Cognee server.
    ///
    /// `access_token` is sent as a bearer token. Local deployments with
    /// authentication disabled may pass `None`.
    ///
    /// # Errors
    ///
    /// Returns an error when `endpoint` is not an HTTP(S) URL.
    pub fn self_hosted(endpoint: &str, access_token: Option<&str>) -> anyhow::Result<Self> {
        Ok(Self {
            inner: RemoteMemory::new(CogneeDialect {
                client: HttpClient::bearer(endpoint, access_token)?,
            }),
        })
    }

    /// Connect to Cognee Cloud using `X-Api-Key` authentication.
    ///
    /// `endpoint` is **your tenant's** base URL, which Cognee Cloud issues per
    /// account and prints on the API-key dashboard — it looks like
    /// `https://tenant-<uuid>.aws.cognee.ai`. There is deliberately no shared
    /// default: this crate carried a `COGNEE_API_ENDPOINT` pointing at
    /// `api.cognee.ai`, and that host answers no TLS handshake at all (its DNS
    /// record resolves, nothing listens), so every "just use the default"
    /// caller met a confusing transport error instead of a working client.
    /// The tenant URL is the only address that exists.
    ///
    /// The tenant and user ids the dashboard shows alongside the URL are not
    /// needed here: the tenant is identified by the hostname, and the API's
    /// only security scheme is this key.
    ///
    /// # Errors
    ///
    /// Returns an error when `endpoint` is invalid or `api_key` is blank.
    pub fn api(endpoint: &str, api_key: &str) -> anyhow::Result<Self> {
        anyhow::ensure!(
            !api_key.trim().is_empty(),
            "cognee API key must not be empty"
        );
        Ok(Self {
            inner: RemoteMemory::new(CogneeDialect {
                client: HttpClient::api_key(endpoint, Some(api_key))?,
            }),
        })
    }
}

#[async_trait]
impl Memory for CogneeMemory {
    /// Returns the Cognee driver identifier.
    fn name(&self) -> &str {
        self.inner.name()
    }
    /// Stores an internally sourced record through the shared contract.
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
    /// Stores a record while preserving its provenance taint.
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
    /// Runs native Cognee recall and applies TinyMemory filters.
    async fn recall(
        &self,
        q: &str,
        l: usize,
        o: RecallOpts<'_>,
    ) -> anyhow::Result<Vec<tinymemory_api::types::MemoryEntry>> {
        self.inner.recall(q, l, o).await
    }
    /// Fetches one exact namespace/key record.
    async fn get(
        &self,
        n: &str,
        k: &str,
    ) -> anyhow::Result<Option<tinymemory_api::types::MemoryEntry>> {
        self.inner.get(n, k).await
    }
    /// Lists records matching the supplied TinyMemory filters.
    async fn list(
        &self,
        n: Option<&str>,
        c: Option<&tinymemory_api::types::MemoryCategory>,
        s: Option<&str>,
    ) -> anyhow::Result<Vec<tinymemory_api::types::MemoryEntry>> {
        self.inner.list(n, c, s).await
    }
    /// Deletes one exact namespace/key record.
    async fn forget(&self, n: &str, k: &str) -> anyhow::Result<bool> {
        self.inner.forget(n, k).await
    }
    /// Summarizes every namespace visible through this adapter.
    async fn namespace_summaries(
        &self,
    ) -> anyhow::Result<Vec<tinymemory_api::types::NamespaceSummary>> {
        self.inner.namespace_summaries().await
    }
    /// Counts every record visible through this adapter.
    async fn count(&self) -> anyhow::Result<usize> {
        self.inner.count().await
    }
    /// Checks whether the configured Cognee service is reachable.
    async fn health_check(&self) -> bool {
        self.inner.health_check().await
    }
    /// Forwarded explicitly: this wrapper delegates method-by-method, so the
    /// defaulted `None` would otherwise shadow `RemoteMemory`'s typed probe —
    /// which is exactly what the first cut shipped, making §U4's deep health
    /// unreachable through every public type (the #68 review's Major 1).
    async fn health_probe(&self) -> Option<tinymemory_api::health::MemoryHealth> {
        self.inner.health_probe().await
    }
}

#[derive(Debug)]
/// Cognee-specific REST operations and wire-format conversion.
struct CogneeDialect {
    client: HttpClient,
}

#[derive(Debug, Clone)]
/// Identity of a Cognee dataset used for one TinyMemory namespace.
struct Dataset {
    id: String,
    name: String,
}

impl CogneeDialect {
    /// Encodes a TinyMemory namespace as a collision-free Cognee dataset name.
    fn dataset_name(namespace: &str) -> String {
        format!("tinymemory__{}", stable_id("dataset", namespace))
    }
    /// Encodes a TinyMemory key as the uploaded envelope's filename.
    fn filename(key: &str) -> String {
        format!("{}.tinymemory.json", stable_id("key", key))
    }

    /// Discovers only datasets owned by the TinyMemory adapter.
    async fn datasets(&self) -> anyhow::Result<Vec<Dataset>> {
        let response: Value = self
            .client
            .json(
                Method::GET,
                "api/v1/datasets/",
                None,
                Attempts::RetryTransient,
            )
            .await?;
        Ok(response
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|value| {
                Some(Dataset {
                    id: value.get("id")?.as_str()?.to_owned(),
                    name: value.get("name")?.as_str()?.to_owned(),
                })
            })
            .filter(|dataset| dataset.name.starts_with("tinymemory__"))
            .collect())
    }

    /// Downloads and decodes every TinyMemory envelope in one dataset.
    async fn dataset_entries(&self, dataset: &Dataset) -> anyhow::Result<Vec<StoredEntry>> {
        let response: Value = self
            .client
            .json(
                Method::GET,
                &format!("api/v1/datasets/{}/data", dataset.id),
                None,
                Attempts::RetryTransient,
            )
            .await?;
        let mut entries = Vec::new();
        for data in response.as_array().into_iter().flatten() {
            let Some(id) = data.get("id").and_then(Value::as_str) else {
                continue;
            };
            let Some(name) = data.get("name").and_then(Value::as_str) else {
                continue;
            };
            // Cognee's text loader strips the final `.json` extension from
            // uploaded filenames; API-shaped test doubles may preserve it.
            if !name.ends_with(".tinymemory") && !name.ends_with(".tinymemory.json") {
                continue;
            }
            let raw = self
                .client
                .text(
                    Method::GET,
                    &format!("api/v1/datasets/{}/data/{id}/raw", dataset.id),
                    Attempts::RetryTransient,
                )
                .await?;
            let mut entry: StoredEntry =
                serde_json::from_str(&raw).context("Cognee record envelope is invalid")?;
            entry.remote_id = format!("{}:{id}", dataset.id);
            if entry.timestamp.is_empty() {
                entry.timestamp = Self::listing_timestamp(data);
            }
            entries.push(entry);
        }
        Ok(entries)
    }

    /// One dataset's data index — id and name per row, NO raw fetches
    /// (issue #69). The listing already carries the uploaded filename, and
    /// this adapter's filenames are deterministic (`Self::filename`), so a
    /// key resolves by matching the name — the per-record raw-fetch loop the
    /// first cut ran existed only because it read the key out of each
    /// envelope body instead.
    /// The listing row's write time. Checked per candidate with `find_map`,
    /// NOT an `or_else` chain over `Value::get`: real Cognee serializes
    /// `"updatedAt": null` for every never-updated record, and `get` on a
    /// present-but-null key answers `Some(Null)` — an `or_else` chain commits
    /// to it and never reaches `createdAt`, emptying every timestamp
    /// (issue #75).
    fn listing_timestamp(data: &Value) -> String {
        ["updatedAt", "updated_at", "createdAt", "created_at"]
            .iter()
            .find_map(|key| data.get(key).and_then(Value::as_str))
            .unwrap_or_default()
            .to_owned()
    }

    async fn data_index(&self, dataset: &Dataset) -> anyhow::Result<Vec<(String, String, String)>> {
        let response: Value = self
            .client
            .json(
                Method::GET,
                &format!("api/v1/datasets/{}/data", dataset.id),
                None,
                Attempts::RetryTransient,
            )
            .await?;
        Ok(response
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|data| {
                Some((
                    data.get("id")?.as_str()?.to_owned(),
                    data.get("name")?.as_str()?.to_owned(),
                    // Kept alongside the id: the envelope this adapter
                    // uploads carries an empty timestamp, so the listing is
                    // the ONLY source a keyed fetch can backfill from (the
                    // enumeration path already does — the keyed path must
                    // agree with it).
                    Self::listing_timestamp(data),
                ))
            })
            .collect())
    }

    /// Resolves a key to its data id by deterministic-filename match:
    /// Cognee's loader strips the final `.json`, so both spellings count.
    /// On a duplicate name (possible only if a historical blind re-add ever
    /// raced), newest-listed wins deterministically — the listing is
    /// insertion-ordered — rather than an arbitrary pick.
    async fn find_data_id(
        &self,
        dataset: &Dataset,
        key: &str,
    ) -> anyhow::Result<Option<(String, String)>> {
        let uploaded = Self::filename(key);
        let stripped = uploaded
            .strip_suffix(".json")
            .unwrap_or(&uploaded)
            .to_owned();
        Ok(self
            .data_index(dataset)
            .await?
            .into_iter()
            .rev()
            .find(|(_, name, _)| *name == uploaded || *name == stripped)
            .map(|(id, _, timestamp)| (id, timestamp)))
    }

    /// Downloads and decodes ONE envelope by its ids, verifying it is the
    /// record asked for — the envelope stays authoritative over the filename
    /// match (a hash collision or a foreign file with our extension must not
    /// serve as someone else's memory).
    async fn fetch_entry(
        &self,
        dataset: &Dataset,
        data_id: &str,
        listing_timestamp: &str,
        namespace: &str,
        key: &str,
    ) -> anyhow::Result<Option<StoredEntry>> {
        let raw = self
            .client
            .text(
                Method::GET,
                &format!("api/v1/datasets/{}/data/{data_id}/raw", dataset.id),
                Attempts::RetryTransient,
            )
            .await?;
        let mut entry: StoredEntry =
            serde_json::from_str(&raw).context("Cognee record envelope is invalid")?;
        if entry.namespace != namespace || entry.key != key {
            anyhow::bail!(
                "Cognee data {data_id} matched key `{key}` by filename but its envelope names \
                 {}/{} — refusing to serve a mismatched record",
                entry.namespace,
                entry.key
            );
        }
        entry.remote_id = format!("{}:{data_id}", dataset.id);
        // The uploaded envelope's timestamp is empty by construction
        // (StoredEntry::new), so without this backfill every keyed get would
        // answer an empty timestamp while the enumeration path answers the
        // listing's — the same record disagreeing with itself.
        if entry.timestamp.is_empty() {
            entry.timestamp = listing_timestamp.to_owned();
        }
        Ok(Some(entry))
    }

    /// Resolves the dataset assigned to a namespace.
    async fn find_dataset(&self, namespace: &str) -> anyhow::Result<Option<Dataset>> {
        let name = Self::dataset_name(namespace);
        Ok(self
            .datasets()
            .await?
            .into_iter()
            .find(|dataset| dataset.name == name))
    }
}

#[async_trait]
impl Dialect for CogneeDialect {
    /// Returns the stable Cognee driver identifier.
    fn name(&self) -> &'static str {
        COGNEE_DRIVER_ID
    }

    /// One namespace = one dataset: entries scoped without the cross-dataset
    /// walk (issue #69). Content lives in the envelopes, so this still pays
    /// one raw per record — that is the documented floor, not a regression.
    async fn namespace_entries(&self, namespace: &str) -> anyhow::Result<Vec<StoredEntry>> {
        match self.find_dataset(namespace).await? {
            Some(dataset) => self.dataset_entries(&dataset).await,
            None => Ok(Vec::new()),
        }
    }

    /// Keyed get in three requests — dataset resolve, one listing, one raw —
    /// however large the store (issue #69: this replaced 1 + D + N serial
    /// requests).
    async fn entry(&self, namespace: &str, key: &str) -> anyhow::Result<Option<StoredEntry>> {
        let Some(dataset) = self.find_dataset(namespace).await? else {
            return Ok(None);
        };
        let Some((data_id, listing_timestamp)) = self.find_data_id(&dataset, key).await? else {
            return Ok(None);
        };
        self.fetch_entry(&dataset, &data_id, &listing_timestamp, namespace, key)
            .await
    }

    /// Replaces an existing envelope and uploads the new exact record.
    async fn upsert(&self, entry: StoredEntry) -> anyhow::Result<()> {
        // Through the keyed seam: dataset + listing, no raw fan-out. The
        // existing record's ids are all the replace path needs. Like delete,
        // the PATCH trusts the dataset-scoped filename match without an
        // envelope read — the name is the key's SHA-256 digest, so a wrong
        // target needs a digest collision, and what a collision would cost
        // here is an overwrite of the colliding record's content with THIS
        // key's envelope (recoverable by that record's next upsert, unlike
        // delete's unrecoverable removal — which is the sharper case and got
        // this same argument first).
        let existing = match self.find_dataset(&entry.namespace).await? {
            Some(dataset) => self
                .find_data_id(&dataset, &entry.key)
                .await?
                .map(|(data_id, _)| (dataset, data_id)),
            None => None,
        };
        let body = serde_json::to_vec(&entry)?;
        let form = multipart::Form::new().part(
            "data",
            multipart::Part::bytes(body)
                .file_name(Self::filename(&entry.key))
                .mime_str("application/json")?,
        );
        let (method, path, form) = if let Some((dataset, data_id)) = existing {
            let dataset_id = dataset.id;
            (
                Method::PATCH,
                format!("api/v1/update?data_id={data_id}&dataset_id={dataset_id}"),
                form,
            )
        } else {
            (
                Method::POST,
                "api/v1/remember".to_owned(),
                form.text("datasetName", Self::dataset_name(&entry.namespace))
                    .text("run_in_background", "false"),
            )
        };
        self.client.send_multipart(method, &path, form).await
    }

    /// Enumerates records across all TinyMemory-owned Cognee datasets.
    async fn entries(&self) -> anyhow::Result<Vec<StoredEntry>> {
        let mut entries = Vec::new();
        for dataset in self.datasets().await? {
            entries.extend(self.dataset_entries(&dataset).await?);
        }
        Ok(entries)
    }

    /// Executes Cognee's native chunk recall and decodes returned envelopes.
    async fn search(
        &self,
        query: &str,
        limit: usize,
        opts: RecallOpts<'_>,
    ) -> anyhow::Result<Vec<StoredEntry>> {
        // Resolve the dataset BEFORE asking recall (issue #75): real Cognee
        // 404s ("No datasets found") when a named dataset resolves to
        // nothing, so the first recall in a fresh namespace — before its
        // first store — errored where every sibling op answers empty. One
        // extra listing request, the same price entry()/delete() pay.
        let datasets = match opts.namespace {
            Some(namespace) => match self.find_dataset(namespace).await? {
                Some(dataset) => Some(vec![dataset.name]),
                None => return Ok(Vec::new()),
            },
            None => None,
        };
        let response: Value = self
            .client
            .json(
                Method::POST,
                "api/v1/recall",
                Some(&json!({
                    "query": query,
                    "search_type": "CHUNKS",
                    "datasets": datasets,
                    "top_k": limit,
                    "only_context": true,
                    "session_id": opts.session_id
                })),
                Attempts::RetryTransient,
            )
            .await?;
        let mut entries = Vec::new();
        for value in response.as_array().into_iter().flatten() {
            let text = value
                .get("text")
                .or_else(|| value.get("content"))
                .or_else(|| value.get("result_object"))
                .and_then(Value::as_str);
            if let Some(text) = text {
                if let Ok(mut entry) = serde_json::from_str::<StoredEntry>(text) {
                    entry.score = value.get("score").and_then(Value::as_f64);
                    entries.push(entry);
                } else {
                    // CHUNKS recall may coalesce adjacent source documents into
                    // newline-delimited text. Each source remains a complete
                    // TinyMemory envelope, so decode them independently.
                    for line in text.lines() {
                        if let Ok(mut entry) = serde_json::from_str::<StoredEntry>(line) {
                            entry.score = value.get("score").and_then(Value::as_f64);
                            entries.push(entry);
                        }
                    }
                }
            }
        }
        Ok(entries)
    }

    /// Finds and deletes an exact TinyMemory logical record.
    async fn delete(&self, namespace: &str, key: &str) -> anyhow::Result<bool> {
        // Three requests, no raw fan-out (issue #69): the filename match
        // resolves the id, and delete needs nothing from the envelope.
        // Deliberately weaker than `fetch_entry`'s envelope check: the
        // filename is the key's SHA-256 digest and the dataset scopes the
        // namespace, so a wrong-record match would need a digest collision —
        // and verifying the envelope would cost exactly the raw fetch this
        // path exists to avoid.
        let Some(dataset) = self.find_dataset(namespace).await? else {
            return Ok(false);
        };
        let Some((data_id, _)) = self.find_data_id(&dataset, key).await? else {
            return Ok(false);
        };
        self.client
            .empty(
                Method::DELETE,
                &format!("api/v1/datasets/{}/data/{data_id}", dataset.id),
                None,
            )
            .await?;
        Ok(true)
    }

    /// Probes Cognee's aggregate health endpoint, typed.
    async fn health(&self) -> anyhow::Result<()> {
        self.client.probe("health").await
    }

    /// Context-only recall carries no score field — see the trait doc for
    /// what this means for `min_score` (documented-inert, not everything-
    /// dropping; the first cut's over-fetch pulled 3x the data and discarded
    /// all of it).
    fn scores_recall(&self) -> bool {
        false
    }
}

#[cfg(test)]
#[path = "cognee_tests.rs"]
mod test;
