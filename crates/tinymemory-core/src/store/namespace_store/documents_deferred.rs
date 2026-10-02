//! Document writes that do not wait on the embedding provider.
//!
//! [`UnifiedMemory::upsert_document_presanitized`] embeds a document's chunks
//! and only then commits the row, so a caller waits out one provider round
//! trip per write. For a conversational autosave that wait sits on the end of
//! every agent turn, and when the provider is slow or refusing (an expired key,
//! an unreachable endpoint) the whole wait is spent on a vector nobody will
//! read before the next turn.
//!
//! This path commits the row and its chunks first, vector-less, and returns.
//! The vectors are computed by a background task and attached afterwards. A
//! vector-less chunk is an ordinary state — it is what an embedding failure
//! already leaves behind — so it stays keyword-searchable throughout, and only
//! semantic recall of the document lags by one provider round trip.
//!
//! # Races
//!
//! Between the commit and the attach the document can be rewritten or
//! forgotten. The attach is one `UPDATE` per chunk, keyed on the chunk id **and
//! its text**, and only touches a chunk that still has no vector. A rewrite
//! replaces the chunks, so the stale vector finds nothing to attach to; a
//! rewrite that happens to keep a chunk's text keeps its id, and the vector is
//! still the right one for it.

use rusqlite::params;
use tokio::task::JoinHandle;

use crate::store::types::NamespaceDocumentInput;

use super::documents::DOCUMENT_CHUNK_MAX_TOKENS;
use super::UnifiedMemory;

/// A document committed without vectors, and the task computing them.
pub(crate) struct DeferredWrite {
    /// The stored document's id.
    pub(crate) document_id: String,
    /// Resolves to the number of chunks that received a vector. Dropping it
    /// detaches the task; the vectors still land.
    pub(crate) vectors: JoinHandle<usize>,
}

impl UnifiedMemory {
    /// [`Self::upsert_document_presanitized`] without the embedding wait.
    ///
    /// Takes already-sanitized input — same contract as that method; go through
    /// `UnifiedMemory::upsert_document_deferred` instead.
    pub(crate) async fn upsert_document_deferred_presanitized(
        &self,
        input: NamespaceDocumentInput,
    ) -> Result<DeferredWrite, String> {
        let namespace = Self::sanitize_namespace(&input.namespace);
        let chunks = Self::chunk_document_content(&input.content, DOCUMENT_CHUNK_MAX_TOKENS);
        let unembedded = vec![None; chunks.len()];
        let document_id = self
            .write_document_presanitized(input, chunks.clone(), unembedded)
            .await?;

        let embedder = std::sync::Arc::clone(&self.embedder);
        let conn = std::sync::Arc::clone(&self.conn);
        let id = document_id.clone();
        let vectors = tokio::spawn(async move {
            let texts: Vec<&str> = chunks.iter().map(String::as_str).collect();
            let embedded = Self::embed_texts_with(embedder.as_ref(), &texts).await;
            let signature = embedder.signature();
            let conn = conn.lock();
            let mut attached = 0;
            for (idx, (text, vector)) in chunks.iter().zip(embedded).enumerate() {
                let Some(vector) = vector else { continue };
                let result = conn.execute(
                    "UPDATE vector_chunks
                        SET embedding = ?1, model_signature = ?2, dim = ?3
                      WHERE namespace = ?4 AND document_id = ?5 AND chunk_id = ?6
                        AND text = ?7 AND embedding IS NULL",
                    params![
                        Self::vec_to_bytes(&vector),
                        signature,
                        vector.len() as i64,
                        namespace,
                        id,
                        format!("{id}:{idx}"),
                        text
                    ],
                );
                match result {
                    Ok(changed) => attached += changed,
                    Err(e) => log::warn!(
                        "[memory] attaching deferred vector failed document_id={id} chunk={idx}: {e}"
                    ),
                }
            }
            log::debug!(
                "[memory] deferred embedding attached {attached}/{} chunk vector(s) document_id={id}",
                chunks.len()
            );
            attached
        });
        Ok(DeferredWrite {
            document_id,
            vectors,
        })
    }
}

#[cfg(test)]
#[path = "documents_deferred_tests.rs"]
mod tests;
