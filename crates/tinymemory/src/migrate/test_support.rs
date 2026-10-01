//! A provider for the migrate tests: the reference driver's keyed records,
//! plus in-memory documents, goals, profile, episodic record and chunks, and
//! an ingest that keeps what it is sent. A family is served only when the
//! test turns it on, so every step's "does not serve" path is reachable.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::unnecessary_literal_bound
)]

use std::collections::BTreeMap;
use std::sync::Mutex;

use async_trait::async_trait;
use tinymemory_conformance::InMemoryProvider;

use crate::capabilities::{Capabilities, Capability};
use crate::chunks::Chunk;
use crate::error::MemoryError;
use crate::goals::GoalsDoc;
use crate::health::MemoryHealth;
use crate::provider::types::{
    ExportPage, ExportRecord, ImportOutcome, IngestItem, IngestOutcome, SourceScope,
};
use crate::provider::{
    ChunkDetail, ChunkEmbedding, ChunkQuery, ConversationSegment, EpisodicEvent,
    EpisodicExportPage, EpisodicImportOutcome, EpisodicPart, EpisodicRecords, EpisodicTurn,
    FacetType, MemoryChunks, MemoryCore, MemoryDocuments, MemoryEpisodicPortability, MemoryGoals,
    MemoryIngest, MemoryPortability, MemoryProfile, MemoryProvider, MemoryRecall, ProfileFacet,
    SegmentEmbedding, SourceTotal, TurnIdRemap, UserState,
};
use crate::recall::OwnedRecallOpts;
use crate::types::{
    MemoryCategory, MemoryEntry, NamespaceDocumentInput, NamespaceRetrievalContext,
    NamespaceSummary, StoredMemoryDocument,
};

/// Where the fake hands out fresh turn ids.
pub(super) const FRESH_IDS: i64 = 1_000_000;

#[derive(Default)]
pub(super) struct Fake {
    core: InMemoryProvider,
    serves: Mutex<Capabilities>,
    pub(super) documents: Mutex<BTreeMap<(String, String), StoredMemoryDocument>>,
    pub(super) puts: Mutex<usize>,
    pub(super) goals: Mutex<GoalsDoc>,
    pub(super) facets: Mutex<BTreeMap<String, ProfileFacet>>,
    pub(super) turns: Mutex<BTreeMap<i64, EpisodicTurn>>,
    pub(super) segments: Mutex<BTreeMap<String, ConversationSegment>>,
    pub(super) events: Mutex<BTreeMap<String, EpisodicEvent>>,
    pub(super) embeddings: Mutex<BTreeMap<(String, String), SegmentEmbedding>>,
    pub(super) chunks: Mutex<Vec<(Chunk, String)>>,
    pub(super) ingested: Mutex<Vec<(&'static str, Vec<IngestItem>)>>,
}

impl Fake {
    /// A fake serving `families` beside the mandatory three.
    pub(super) fn serving(families: &[Capability]) -> Self {
        let fake = Self::default();
        *fake.serves.lock().unwrap() = Capabilities::mandatory().with_all(families);
        fake
    }

    fn has(&self, family: Capability) -> bool {
        self.serves.lock().unwrap().contains(family)
    }
}

trait WithAll {
    fn with_all(self, families: &[Capability]) -> Self;
}

impl WithAll for Capabilities {
    fn with_all(mut self, families: &[Capability]) -> Self {
        for family in families {
            self.insert(*family);
        }
        self
    }
}

#[async_trait]
impl MemoryCore for Fake {
    async fn store(
        &self,
        namespace: &str,
        key: &str,
        content: &str,
        category: MemoryCategory,
        session_id: Option<&str>,
        taint: crate::types::MemoryTaint,
    ) -> Result<(), MemoryError> {
        self.core
            .store(namespace, key, content, category, session_id, taint)
            .await
    }

    async fn get(&self, namespace: &str, key: &str) -> Result<Option<MemoryEntry>, MemoryError> {
        self.core.get(namespace, key).await
    }

    async fn forget(&self, namespace: &str, key: &str) -> Result<bool, MemoryError> {
        self.core.forget(namespace, key).await
    }

    async fn list(
        &self,
        namespace: Option<&str>,
        category: Option<&MemoryCategory>,
        session_id: Option<&str>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        self.core.list(namespace, category, session_id).await
    }

    async fn namespaces(&self) -> Result<Vec<NamespaceSummary>, MemoryError> {
        self.core.namespaces().await
    }
}

#[async_trait]
impl MemoryRecall for Fake {
    async fn recall(
        &self,
        query: &str,
        limit: usize,
        opts: &OwnedRecallOpts,
        scope: Option<&SourceScope>,
    ) -> Result<Vec<MemoryEntry>, MemoryError> {
        self.core.recall(query, limit, opts, scope).await
    }
}

#[async_trait]
impl MemoryPortability for Fake {
    async fn export_page(
        &self,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<ExportPage, MemoryError> {
        self.core.export_page(cursor, limit).await
    }

    async fn import_records(
        &self,
        records: Vec<ExportRecord>,
    ) -> Result<ImportOutcome, MemoryError> {
        self.core.import_records(records).await
    }
}

#[async_trait]
impl MemoryProvider for Fake {
    fn driver_id(&self) -> &str {
        "fake"
    }

    fn capabilities(&self) -> Capabilities {
        *self.serves.lock().unwrap()
    }

    async fn health(&self) -> MemoryHealth {
        MemoryHealth::Ready
    }

    fn as_documents(&self) -> Option<&dyn MemoryDocuments> {
        self.has(Capability::Documents)
            .then_some(self as &dyn MemoryDocuments)
    }

    fn as_goals(&self) -> Option<&dyn MemoryGoals> {
        self.has(Capability::Goals)
            .then_some(self as &dyn MemoryGoals)
    }

    fn as_profile(&self) -> Option<&dyn MemoryProfile> {
        self.has(Capability::Profile)
            .then_some(self as &dyn MemoryProfile)
    }

    fn as_episodic_portability(&self) -> Option<&dyn MemoryEpisodicPortability> {
        self.has(Capability::EpisodicPortability)
            .then_some(self as &dyn MemoryEpisodicPortability)
    }

    fn as_chunks(&self) -> Option<&dyn MemoryChunks> {
        self.has(Capability::Chunks)
            .then_some(self as &dyn MemoryChunks)
    }

    fn as_ingest(&self) -> Option<&dyn MemoryIngest> {
        self.has(Capability::Ingest)
            .then_some(self as &dyn MemoryIngest)
    }
}

fn unused<T>() -> Result<T, MemoryError> {
    Err(MemoryError::Other(anyhow::anyhow!("not used by a copy")))
}

#[async_trait]
impl MemoryDocuments for Fake {
    async fn put_document(&self, input: NamespaceDocumentInput) -> Result<String, MemoryError> {
        *self.puts.lock().unwrap() += 1;
        let id = input
            .document_id
            .clone()
            .unwrap_or_else(|| format!("{}:{}", input.namespace, input.key));
        let stored = StoredMemoryDocument {
            document_id: id.clone(),
            namespace: input.namespace.clone(),
            key: input.key.clone(),
            title: input.title,
            content: input.content,
            source_type: input.source_type,
            priority: input.priority,
            tags: input.tags,
            metadata: input.metadata,
            category: input.category,
            session_id: input.session_id,
            created_at: 1.0,
            updated_at: 1.0,
            markdown_rel_path: String::new(),
            taint: input.taint,
        };
        self.documents
            .lock()
            .unwrap()
            .insert((input.namespace, input.key), stored);
        Ok(id)
    }

    async fn get_document(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<StoredMemoryDocument>, MemoryError> {
        Ok(self
            .documents
            .lock()
            .unwrap()
            .get(&(namespace.to_string(), key.to_string()))
            .cloned())
    }

    async fn list_documents(
        &self,
        namespace: Option<&str>,
    ) -> Result<serde_json::Value, MemoryError> {
        let documents: Vec<serde_json::Value> = self
            .documents
            .lock()
            .unwrap()
            .values()
            .filter(|d| namespace.is_none_or(|n| d.namespace == n))
            .map(|d| serde_json::json!({"namespace": d.namespace, "key": d.key}))
            .collect();
        Ok(serde_json::json!({"count": documents.len(), "documents": documents}))
    }

    async fn list_namespaces(&self) -> Result<Vec<String>, MemoryError> {
        Ok(self
            .documents
            .lock()
            .unwrap()
            .keys()
            .map(|(namespace, _)| namespace.clone())
            .collect())
    }

    async fn delete_document(
        &self,
        _namespace: &str,
        _document_id: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        unused()
    }

    async fn clear_namespace(&self, _namespace: &str) -> Result<(), MemoryError> {
        unused()
    }

    async fn query_documents(
        &self,
        _namespace: &str,
        _query: &str,
        _limit: usize,
    ) -> Result<NamespaceRetrievalContext, MemoryError> {
        unused()
    }
}

#[async_trait]
impl MemoryGoals for Fake {
    async fn goals(&self) -> Result<GoalsDoc, MemoryError> {
        Ok(self.goals.lock().unwrap().clone())
    }

    async fn set_goals(&self, goals: GoalsDoc) -> Result<(), MemoryError> {
        *self.goals.lock().unwrap() = goals;
        Ok(())
    }
}

#[async_trait]
impl MemoryProfile for Fake {
    async fn list_active_facets(&self) -> Result<Vec<ProfileFacet>, MemoryError> {
        unused()
    }

    async fn list_all_facets(&self) -> Result<Vec<ProfileFacet>, MemoryError> {
        Ok(self.facets.lock().unwrap().values().cloned().collect())
    }

    async fn get_facet(&self, key: &str) -> Result<Option<ProfileFacet>, MemoryError> {
        Ok(self.facets.lock().unwrap().get(key).cloned())
    }

    async fn facets_by_type(
        &self,
        _facet_type: FacetType,
    ) -> Result<Vec<ProfileFacet>, MemoryError> {
        unused()
    }

    async fn upsert_facet(&self, facet: &ProfileFacet) -> Result<(), MemoryError> {
        self.facets
            .lock()
            .unwrap()
            .insert(facet.key.clone(), facet.clone());
        Ok(())
    }

    async fn upsert_provider_facet(
        &self,
        _facet_id: &str,
        _facet_type: FacetType,
        _key: &str,
        _value: &str,
        _confidence: f64,
        _segment_id: Option<&str>,
        _observed_at: f64,
    ) -> Result<(), MemoryError> {
        unused()
    }

    async fn set_facet_user_state(
        &self,
        _key: &str,
        _user_state: UserState,
    ) -> Result<bool, MemoryError> {
        unused()
    }

    async fn delete_facet(&self, _key: &str) -> Result<bool, MemoryError> {
        unused()
    }

    async fn delete_facet_by_id(&self, _facet_id: &str) -> Result<bool, MemoryError> {
        unused()
    }

    async fn drop_facets_below(&self, _threshold: f64) -> Result<usize, MemoryError> {
        unused()
    }

    async fn workflow_identity_matches(&self, _key_pattern: &str, _canonical_value: &str) -> bool {
        false
    }
}

/// One page of `map` after `cursor`, keyed by `key`.
fn page_of<K: Ord + Clone + std::fmt::Display + std::str::FromStr, V: Clone>(
    map: &BTreeMap<K, V>,
    cursor: Option<&str>,
    limit: usize,
) -> (Vec<V>, Option<String>) {
    let after = cursor.and_then(|c| c.parse::<K>().ok());
    let mut rest = map
        .iter()
        .filter(|(k, _)| after.as_ref().is_none_or(|a| *k > a))
        .peekable();
    let mut taken = Vec::new();
    let mut last = None;
    while taken.len() < limit {
        let Some((k, v)) = rest.next() else { break };
        taken.push(v.clone());
        last = Some(k.to_string());
    }
    let next = rest.peek().is_some().then_some(last).flatten();
    (taken, next)
}

/// Counts one record an import wrote, or found already there.
fn count(outcome: &mut EpisodicImportOutcome, changed: bool) {
    if changed {
        outcome.imported += 1;
    } else {
        outcome.skipped += 1;
    }
}

#[async_trait]
impl MemoryEpisodicPortability for Fake {
    async fn export_episodic(
        &self,
        part: EpisodicPart,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<EpisodicExportPage, MemoryError> {
        let (records, next_cursor) = match part {
            EpisodicPart::Turns => {
                let (turns, next) = page_of(&self.turns.lock().unwrap(), cursor, limit);
                (EpisodicRecords::Turns(turns), next)
            }
            EpisodicPart::Segments => {
                let (segments, next) = page_of(&self.segments.lock().unwrap(), cursor, limit);
                (EpisodicRecords::Segments(segments), next)
            }
            EpisodicPart::Events => {
                let (events, next) = page_of(&self.events.lock().unwrap(), cursor, limit);
                (EpisodicRecords::Events(events), next)
            }
            EpisodicPart::SegmentEmbeddings => {
                let flat: BTreeMap<String, SegmentEmbedding> = self
                    .embeddings
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|((s, m), v)| (format!("{s}/{m}"), v.clone()))
                    .collect();
                let (embeddings, next) = page_of(&flat, cursor, limit);
                (EpisodicRecords::SegmentEmbeddings(embeddings), next)
            }
        };
        Ok(EpisodicExportPage {
            records,
            next_cursor,
        })
    }

    async fn import_episodic(
        &self,
        records: EpisodicRecords,
    ) -> Result<EpisodicImportOutcome, MemoryError> {
        let mut outcome = EpisodicImportOutcome::default();
        match records {
            EpisodicRecords::Turns(turns) => {
                let mut held = self.turns.lock().unwrap();
                for turn in turns {
                    let id = turn.id.expect("an exported turn has an id");
                    match held.get(&id) {
                        Some(existing) if *existing == turn => outcome.skipped += 1,
                        None => {
                            held.insert(id, turn);
                            outcome.imported += 1;
                        }
                        Some(_) => {
                            let elsewhere = held.iter().find(|(at, t)| {
                                **at != id
                                    && EpisodicTurn {
                                        id: Some(id),
                                        ..(*t).clone()
                                    } == turn
                            });
                            if let Some((at, _)) = elsewhere {
                                outcome.skipped += 1;
                                outcome.remapped.push(TurnIdRemap { from: id, to: *at });
                                continue;
                            }
                            let to = held.keys().max().copied().unwrap_or(0).max(FRESH_IDS) + 1;
                            held.insert(
                                to,
                                EpisodicTurn {
                                    id: Some(to),
                                    ..turn
                                },
                            );
                            outcome.imported += 1;
                            outcome.remapped.push(TurnIdRemap { from: id, to });
                        }
                    }
                }
            }
            EpisodicRecords::Segments(segments) => {
                let mut held = self.segments.lock().unwrap();
                for segment in segments {
                    let changed = held.get(&segment.segment_id) != Some(&segment);
                    held.insert(segment.segment_id.clone(), segment);
                    count(&mut outcome, changed);
                }
            }
            EpisodicRecords::Events(events) => {
                let mut held = self.events.lock().unwrap();
                for event in events {
                    let changed = held.get(&event.event_id) != Some(&event);
                    held.insert(event.event_id.clone(), event);
                    count(&mut outcome, changed);
                }
            }
            EpisodicRecords::SegmentEmbeddings(embeddings) => {
                let mut held = self.embeddings.lock().unwrap();
                for embedding in embeddings {
                    let key = (
                        embedding.segment_id.clone(),
                        embedding.model_signature.clone(),
                    );
                    let changed = held.get(&key) != Some(&embedding);
                    held.insert(key, embedding);
                    count(&mut outcome, changed);
                }
            }
        }
        Ok(outcome)
    }
}

#[async_trait]
impl MemoryChunks for Fake {
    async fn list_chunks(
        &self,
        query: &ChunkQuery,
        _scope: Option<&SourceScope>,
    ) -> Result<Vec<Chunk>, MemoryError> {
        // Newest first, as the contract orders a listing.
        let mut rows: Vec<Chunk> = self
            .chunks
            .lock()
            .unwrap()
            .iter()
            .map(|(chunk, _)| chunk.clone())
            .filter(|c| {
                query
                    .source_kind
                    .is_none_or(|k| c.metadata.source_kind == k)
            })
            .filter(|c| {
                query
                    .source_id
                    .as_deref()
                    .is_none_or(|id| c.metadata.source_id == id)
            })
            .collect();
        rows.sort_by_key(|c| std::cmp::Reverse(c.metadata.timestamp));
        let offset = query.offset.unwrap_or(0);
        let limit = query.limit.unwrap_or(100);
        Ok(rows.into_iter().skip(offset).take(limit).collect())
    }

    async fn source_totals(
        &self,
        _limit: usize,
        _scope: Option<&SourceScope>,
    ) -> Result<Vec<SourceTotal>, MemoryError> {
        let mut totals: BTreeMap<(String, String), SourceTotal> = BTreeMap::new();
        for (chunk, _) in self.chunks.lock().unwrap().iter() {
            let key = (
                chunk.metadata.source_kind.as_str().to_string(),
                chunk.metadata.source_id.clone(),
            );
            let total = totals.entry(key).or_insert(SourceTotal {
                source_kind: chunk.metadata.source_kind,
                source_id: chunk.metadata.source_id.clone(),
                chunk_count: 0,
                most_recent_ms: 0,
            });
            total.chunk_count += 1;
        }
        Ok(totals.into_values().collect())
    }

    async fn get_chunk(&self, _chunk_id: &str) -> Result<Option<Chunk>, MemoryError> {
        unused()
    }

    async fn chunk_detail(&self, chunk_id: &str) -> Result<Option<ChunkDetail>, MemoryError> {
        Ok(self
            .chunks
            .lock()
            .unwrap()
            .iter()
            .find(|(chunk, _)| chunk.id == chunk_id)
            .map(|(chunk, body)| ChunkDetail {
                chunk: chunk.clone(),
                body: Some(body.clone()),
                content_path: None,
                lifecycle_status: None,
                has_embedding: false,
            }))
    }

    async fn storage_kinds(&self) -> Result<Vec<String>, MemoryError> {
        unused()
    }

    async fn chunk_embeddings(
        &self,
        _chunk_ids: &[String],
        _model_signature: &str,
    ) -> Result<Vec<ChunkEmbedding>, MemoryError> {
        unused()
    }
}

#[async_trait]
impl MemoryIngest for Fake {
    async fn ingest_document(&self, item: IngestItem) -> Result<IngestOutcome, MemoryError> {
        self.ingested.lock().unwrap().push(("document", vec![item]));
        Ok(IngestOutcome {
            written: 1,
            ..IngestOutcome::default()
        })
    }

    async fn ingest_chat(&self, messages: Vec<IngestItem>) -> Result<IngestOutcome, MemoryError> {
        let written = u32::try_from(messages.len()).unwrap();
        self.ingested.lock().unwrap().push(("chat", messages));
        Ok(IngestOutcome {
            written,
            ..IngestOutcome::default()
        })
    }

    async fn ingest_email(&self, messages: Vec<IngestItem>) -> Result<IngestOutcome, MemoryError> {
        let written = u32::try_from(messages.len()).unwrap();
        self.ingested.lock().unwrap().push(("email", messages));
        Ok(IngestOutcome {
            written,
            ..IngestOutcome::default()
        })
    }
}
