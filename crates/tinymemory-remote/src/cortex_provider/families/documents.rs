//! `MemoryDocuments` over the hosted wire.
//!
//! A document is two records under its key:
//!
//! - **Content**: the namespace's own keyed record, carrying the document's id
//!   in its provenance. Documents and keyed records therefore share one
//!   keyspace, and `get(namespace, key)` returns a document's body.
//! - **Details**: title, source type, priority, tags, metadata and times, as an
//!   inert bookkeeping record in `tmi:documents/<namespace scope>`.
//!
//! Details apply only while the content still carries their document id, so a
//! later plain `store` of the key turns the document back into an ordinary
//! record. Content that carries an id without details is still a document,
//! with default details.

use std::collections::HashMap;

use async_trait::async_trait;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tinymemory_api::error::MemoryError;
use tinymemory_api::mandatory::engine_error;
use tinymemory_api::provider::MemoryDocuments;
use tinymemory_api::types::{
    MemoryItemKind, MemoryTaint, NamespaceDocumentInput, NamespaceMemoryHit,
    NamespaceRetrievalContext, RetrievalScoreBreakdown, StoredMemoryDocument,
};

use super::records::{Place, Provenance, Record, Records, Version};
use super::relevance::{estimate, freshness};
use super::scopes::{details_owner, document_details, DOCUMENT_DETAILS};
use crate::common::Attempts;
use crate::cortex::{CortexDialect, Route};
use crate::cortex_labels::digest;
use crate::cortex_provider::CortexProvider;

/// A document's details, as its bookkeeping record holds them.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Details {
    document_id: String,
    title: String,
    source_type: String,
    priority: String,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    metadata: Value,
    /// Seconds since the Unix epoch.
    created_at: f64,
    /// Seconds since the Unix epoch.
    updated_at: f64,
}

impl Details {
    /// The details a document has when its details record is missing.
    fn fallback(key: &str, document_id: &str) -> Self {
        Self {
            document_id: document_id.to_string(),
            title: key.to_string(),
            source_type: "document".to_string(),
            priority: "medium".to_string(),
            tags: Vec::new(),
            metadata: Value::Null,
            created_at: 0.0,
            updated_at: 0.0,
        }
    }
}

/// One live document: its content version and the details that apply to it.
#[derive(Clone, Debug)]
struct Document {
    namespace: String,
    content: Version,
    details: Details,
}

impl Document {
    fn id(&self) -> &str {
        &self.details.document_id
    }

    fn stored(&self) -> StoredMemoryDocument {
        let record = &self.content.record;
        StoredMemoryDocument {
            document_id: self.details.document_id.clone(),
            namespace: self.namespace.clone(),
            key: record.key.clone(),
            title: self.details.title.clone(),
            content: record.content.clone(),
            source_type: self.details.source_type.clone(),
            priority: self.details.priority.clone(),
            tags: self.details.tags.clone(),
            metadata: self.details.metadata.clone(),
            category: record.category.to_string(),
            session_id: record.session_id.clone(),
            created_at: self.details.created_at,
            updated_at: self.details.updated_at,
            // Hosted memory keeps no markdown mirror on disk.
            markdown_rel_path: String::new(),
            taint: record.taint,
        }
    }

    fn row(&self) -> Value {
        json!({
            "documentId": self.details.document_id,
            "namespace": self.namespace,
            "key": self.content.record.key,
            "title": self.details.title,
            "sourceType": self.details.source_type,
            "priority": self.details.priority,
            "createdAt": self.details.created_at,
            "updatedAt": self.details.updated_at,
            "taint": taint_name(self.content.record.taint),
        })
    }

    fn hit(&self, score: f64, score_breakdown: RetrievalScoreBreakdown) -> NamespaceMemoryHit {
        let record = &self.content.record;
        NamespaceMemoryHit {
            id: self.details.document_id.clone(),
            kind: MemoryItemKind::Document,
            namespace: self.namespace.clone(),
            key: record.key.clone(),
            title: Some(self.details.title.clone()),
            content: record.content.clone(),
            category: record.category.to_string(),
            source_type: Some(self.details.source_type.clone()),
            updated_at: self.details.updated_at,
            score,
            score_breakdown,
            document_id: Some(self.details.document_id.clone()),
            chunk_id: None,
            supporting_relations: Vec::new(),
            taint: record.taint,
        }
    }
}

/// A taint's name in a listing row.
fn taint_name(taint: MemoryTaint) -> Value {
    serde_json::to_value(taint).unwrap_or_else(|_| json!("internal"))
}

/// Seconds since the Unix epoch, now.
fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs_f64())
        .unwrap_or_default()
}

/// The id a new document gets when the caller names none: stable for its
/// namespace and key, so a rewrite of a lost document keeps its id.
fn derived_id(namespace: &str, key: &str) -> String {
    format!("doc-{}", digest(&format!("{namespace}\u{0}{key}")))
}

/// The context text rendered from `hits`, as the embedded engine renders it.
fn context_text(hits: &[NamespaceMemoryHit], query: Option<&str>) -> String {
    let mut parts = Vec::new();
    if let Some(query) = query {
        parts.push(format!("Query: {query}"));
    }
    for hit in hits {
        let title = hit.title.clone().unwrap_or_else(|| hit.key.clone());
        parts.push(format!("{title}: {}", hit.content.trim()));
    }
    parts.join("\n\n")
}

/// The two places a namespace's documents live.
struct Places {
    content: Place,
    details: Place,
}

impl CortexProvider {
    fn document_places(&self, namespace: &str) -> Result<Places, MemoryError> {
        let content = Place::namespace(&self.dialect, namespace).map_err(engine_error)?;
        let details = Place::bookkeeping(&self.dialect, document_details(&content.scope))
            .map_err(engine_error)?;
        Ok(Places { content, details })
    }

    /// Every live document in `namespace`, newest first.
    async fn documents_in(&self, namespace: &str) -> Result<Vec<Document>, MemoryError> {
        let places = self.document_places(namespace)?;
        let records = Records::new(&self.dialect);
        let content = records
            .live_all(&places.content)
            .await
            .map_err(engine_error)?;
        let details: HashMap<String, Details> = records
            .live_all(&places.details)
            .await
            .map_err(engine_error)?
            .into_iter()
            .filter_map(|version| {
                let details = serde_json::from_str::<Details>(&version.record.content).ok()?;
                Some((version.record.key, details))
            })
            .collect();
        let mut documents: Vec<Document> = content
            .into_iter()
            .filter_map(|version| {
                let id = version.record.provenance.document.clone()?;
                let details = details
                    .get(&version.record.key)
                    .filter(|details| details.document_id == id)
                    .cloned()
                    .unwrap_or_else(|| Details::fallback(&version.record.key, &id));
                Some(Document {
                    namespace: namespace.to_string(),
                    content: version,
                    details,
                })
            })
            .collect();
        documents.sort_by(|a, b| {
            b.details
                .updated_at
                .partial_cmp(&a.details.updated_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(documents)
    }

    /// The live document under `key` in `namespace`, if there is one.
    async fn document(&self, namespace: &str, key: &str) -> Result<Option<Document>, MemoryError> {
        let places = self.document_places(namespace)?;
        let records = Records::new(&self.dialect);
        let Some(content) = records
            .live(&places.content, key)
            .await
            .map_err(engine_error)?
        else {
            return Ok(None);
        };
        let Some(id) = content.record.provenance.document.clone() else {
            // A plain record: the key holds no document.
            return Ok(None);
        };
        let details = records
            .live(&places.details, key)
            .await
            .map_err(engine_error)?
            .and_then(|version| serde_json::from_str::<Details>(&version.record.content).ok())
            .filter(|details| details.document_id == id)
            .unwrap_or_else(|| Details::fallback(key, &id));
        Ok(Some(Document {
            namespace: namespace.to_string(),
            content,
            details,
        }))
    }

    /// Every namespace whose details scope the account holds.
    async fn namespaces_with_details(&self) -> Result<Vec<String>, MemoryError> {
        let paths = self
            .dialect
            .scope_paths(Some(DOCUMENT_DETAILS))
            .await
            .map_err(engine_error)?;
        let mut namespaces: Vec<String> = paths
            .iter()
            .filter_map(|path| details_owner(path))
            .filter_map(CortexDialect::namespace_of)
            .collect();
        namespaces.sort();
        namespaces.dedup();
        Ok(namespaces)
    }
}

#[async_trait]
impl MemoryDocuments for CortexProvider {
    async fn put_document(&self, input: NamespaceDocumentInput) -> Result<String, MemoryError> {
        if input.namespace.trim().is_empty() || input.key.trim().is_empty() {
            return Err(MemoryError::Invalid(
                "a document needs a namespace and a key".to_string(),
            ));
        }
        let places = self.document_places(&input.namespace)?;
        let existing = self.document(&input.namespace, &input.key).await?;
        let now = now_secs();
        let (document_id, created_at) = match &existing {
            Some(document) => (document.id().to_string(), document.details.created_at),
            None => (
                input
                    .document_id
                    .clone()
                    .filter(|id| !id.trim().is_empty())
                    .unwrap_or_else(|| derived_id(&input.namespace, &input.key)),
                now,
            ),
        };
        let records = Records::new(&self.dialect);
        let content = Record {
            key: input.key.clone(),
            content: input.content.clone(),
            category: crate::common::category(Some(&input.category)),
            session_id: input.session_id.clone(),
            taint: input.taint,
            provenance: Provenance {
                document: Some(document_id.clone()),
                ..Provenance::default()
            },
        };
        records
            .put(&places.content, &content, None)
            .await
            .map_err(engine_error)?;
        let details = Details {
            document_id: document_id.clone(),
            title: input.title,
            source_type: input.source_type,
            priority: input.priority,
            tags: input.tags,
            metadata: input.metadata,
            created_at,
            updated_at: now,
        };
        records
            .put(
                &places.details,
                &Record::plain(input.key, serde_json::to_string(&details)?),
                None,
            )
            .await
            .map_err(engine_error)?;
        Ok(document_id)
    }

    async fn get_document(
        &self,
        namespace: &str,
        key: &str,
    ) -> Result<Option<StoredMemoryDocument>, MemoryError> {
        Ok(self
            .document(namespace, key)
            .await?
            .map(|document| document.stored()))
    }

    async fn list_documents(&self, namespace: Option<&str>) -> Result<Value, MemoryError> {
        let namespaces = match namespace {
            Some(namespace) => vec![namespace.to_string()],
            None => self.namespaces_with_details().await?,
        };
        let mut documents = Vec::new();
        for namespace in &namespaces {
            documents.extend(self.documents_in(namespace).await?);
        }
        documents.sort_by(|a, b| {
            b.details
                .updated_at
                .partial_cmp(&a.details.updated_at)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let rows: Vec<Value> = documents.iter().map(Document::row).collect();
        Ok(json!({ "count": rows.len(), "documents": rows }))
    }

    async fn list_namespaces(&self) -> Result<Vec<String>, MemoryError> {
        let mut out = Vec::new();
        for namespace in self.namespaces_with_details().await? {
            if !self.documents_in(&namespace).await?.is_empty() {
                out.push(namespace);
            }
        }
        Ok(out)
    }

    async fn delete_document(
        &self,
        namespace: &str,
        document_id: &str,
    ) -> Result<Value, MemoryError> {
        let places = self.document_places(namespace)?;
        let found = self
            .documents_in(namespace)
            .await?
            .into_iter()
            .find(|document| document.id() == document_id);
        let deleted = match found {
            Some(document) => {
                let records = Records::new(&self.dialect);
                let key = &document.content.record.key;
                records
                    .remove(&places.content, key)
                    .await
                    .map_err(engine_error)?;
                records
                    .remove(&places.details, key)
                    .await
                    .map_err(engine_error)?;
                true
            }
            None => false,
        };
        Ok(json!({
            "deleted": deleted,
            "namespace": namespace,
            "documentId": document_id,
        }))
    }

    async fn clear_namespace(&self, namespace: &str) -> Result<(), MemoryError> {
        let places = self.document_places(namespace)?;
        let records = Records::new(&self.dialect);
        records.clear(&places.content).await.map_err(engine_error)?;
        records.clear(&places.details).await.map_err(engine_error)?;
        Ok(())
    }

    async fn query_documents(
        &self,
        namespace: &str,
        query: &str,
        limit: usize,
    ) -> Result<NamespaceRetrievalContext, MemoryError> {
        let places = self.document_places(namespace)?;
        let answer: Value = self
            .dialect
            .client
            .json(
                Method::POST,
                self.dialect.wire.path(Route::Recall),
                Some(&json!({ "scope": places.content.scope, "query": query })),
                Attempts::RetryTransient,
            )
            .await
            .map_err(engine_error)?;
        // The engine's order is the ranking; a key's first appearance places it.
        let mut ranked: Vec<String> = Vec::new();
        for version in answer
            .pointer("/layers/events")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Version::of)
        {
            if !ranked.contains(&version.record.key) {
                ranked.push(version.record.key);
            }
        }
        let documents: HashMap<String, Document> = self
            .documents_in(namespace)
            .await?
            .into_iter()
            .map(|document| (document.content.record.key.clone(), document))
            .collect();
        let ranked: Vec<&Document> = ranked.iter().filter_map(|key| documents.get(key)).collect();
        let total = ranked.len();
        let mut hits: Vec<NamespaceMemoryHit> = ranked
            .into_iter()
            .enumerate()
            .map(|(position, document)| {
                let record = &document.content.record;
                let guess = estimate(position, total, query, &record.key, &record.content);
                document.hit(
                    guess.score,
                    RetrievalScoreBreakdown {
                        keyword_relevance: guess.overlap,
                        vector_similarity: guess.rank,
                        final_score: guess.score,
                        ..RetrievalScoreBreakdown::default()
                    },
                )
            })
            .collect();
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        hits.truncate(limit);
        Ok(NamespaceRetrievalContext {
            namespace: namespace.to_string(),
            query: Some(query.to_string()),
            context_text: context_text(&hits, Some(query)),
            hits,
        })
    }

    async fn recall_documents(
        &self,
        namespace: &str,
        limit: usize,
    ) -> Result<NamespaceRetrievalContext, MemoryError> {
        let now = now_secs();
        let mut hits: Vec<NamespaceMemoryHit> = self
            .documents_in(namespace)
            .await?
            .iter()
            .map(|document| {
                let fresh = freshness(now - document.details.updated_at);
                document.hit(
                    fresh,
                    RetrievalScoreBreakdown {
                        freshness: fresh,
                        final_score: fresh,
                        ..RetrievalScoreBreakdown::default()
                    },
                )
            })
            .collect();
        hits.truncate(limit);
        Ok(NamespaceRetrievalContext {
            namespace: namespace.to_string(),
            query: None,
            context_text: context_text(&hits, None),
            hits,
        })
    }
}

#[cfg(test)]
#[path = "documents_test.rs"]
mod test;
