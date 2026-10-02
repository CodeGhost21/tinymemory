use super::*;

use std::sync::Arc;

use serde_json::json;
use tempfile::TempDir;
use tokio::sync::Semaphore;

use crate::store::NamespaceDocumentInput;

/// Embedder whose requests for text containing "slow" wait for a permit, and
/// which can refuse every request, so the order of the commit and the vectors
/// is under the test's control.
struct GatedEmbedder {
    release: Arc<Semaphore>,
    refuse: bool,
}

#[async_trait::async_trait]
impl tinymemory_api::host::EmbeddingProvider for GatedEmbedder {
    fn name(&self) -> &str {
        "gated"
    }

    fn model_id(&self) -> &str {
        "gated-test"
    }

    fn dimensions(&self) -> usize {
        3
    }

    async fn embed(&self, texts: &[&str]) -> anyhow::Result<Vec<Vec<f32>>> {
        if texts.iter().any(|text| text.contains("slow")) {
            self.release.acquire().await.unwrap().forget();
        }
        if self.refuse {
            anyhow::bail!("provider refused");
        }
        Ok(texts.iter().map(|_| vec![0.1, 0.2, 0.3]).collect())
    }
}

fn input(key: &str, content: &str) -> NamespaceDocumentInput {
    NamespaceDocumentInput {
        namespace: "test:deferred".to_string(),
        key: key.to_string(),
        title: key.to_string(),
        content: content.to_string(),
        source_type: "chat".to_string(),
        priority: "medium".to_string(),
        tags: vec![],
        metadata: json!({}),
        category: "conversation".to_string(),
        session_id: None,
        document_id: None,
        taint: crate::MemoryTaint::Internal,
    }
}

fn chunk_rows(memory: &UnifiedMemory, document_id: &str) -> Vec<(String, bool)> {
    let conn = memory.conn.lock();
    let mut stmt = conn
        .prepare(
            "SELECT text, embedding IS NOT NULL FROM vector_chunks
              WHERE document_id = ?1 ORDER BY chunk_id",
        )
        .unwrap();
    stmt.query_map([document_id], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn memory_with(refuse: bool) -> (TempDir, UnifiedMemory, Arc<Semaphore>) {
    let tmp = TempDir::new().unwrap();
    let release = Arc::new(Semaphore::new(0));
    let embedder = Arc::new(GatedEmbedder {
        release: Arc::clone(&release),
        refuse,
    });
    let memory = UnifiedMemory::new(tmp.path(), embedder, None).unwrap();
    (tmp, memory, release)
}

#[tokio::test]
async fn returns_before_the_provider_answers_then_attaches_the_vectors() {
    let (_tmp, memory, release) = memory_with(false);

    let written = memory
        .upsert_document_deferred(input("k", "slow body"))
        .await
        .unwrap();
    // Committed and keyword-visible while the provider is still being waited on.
    assert_eq!(
        chunk_rows(&memory, &written.document_id),
        vec![("slow body".to_string(), false)]
    );

    release.add_permits(1);
    assert_eq!(written.vectors.await.unwrap(), 1);
    assert_eq!(
        chunk_rows(&memory, &written.document_id),
        vec![("slow body".to_string(), true)]
    );
}

#[tokio::test]
async fn a_refusing_provider_leaves_the_stored_document_vectorless() {
    let (_tmp, memory, _release) = memory_with(true);

    let written = memory
        .upsert_document_deferred(input("k", "plain body"))
        .await
        .unwrap();

    assert_eq!(written.vectors.await.unwrap(), 0);
    assert_eq!(
        chunk_rows(&memory, &written.document_id),
        vec![("plain body".to_string(), false)]
    );
}

#[tokio::test]
async fn a_rewrite_in_the_meantime_does_not_receive_the_stale_vector() {
    let (_tmp, memory, release) = memory_with(false);

    let first = memory
        .upsert_document_deferred(input("k", "slow first"))
        .await
        .unwrap();
    // Same key, new content: replaces the chunks while the first embedding is
    // still in flight.
    let second = memory.upsert_document(input("k", "second")).await.unwrap();
    assert_eq!(first.document_id, second);

    release.add_permits(1);
    assert_eq!(first.vectors.await.unwrap(), 0);
    assert_eq!(
        chunk_rows(&memory, &second),
        vec![("second".to_string(), true)]
    );
}

#[tokio::test]
async fn the_write_gate_still_rejects_a_secret_looking_key() {
    let (_tmp, memory, _release) = memory_with(false);

    // Assembled at runtime so no credential-shaped literal sits in the source.
    let secret_looking_key = format!("sk-ant-{}", "api03-abcdefghijklmnopqrstuvwxyz0123456789");
    let rejected = memory
        .upsert_document_deferred(input(&secret_looking_key, "x"))
        .await;

    assert!(rejected.is_err());
}

#[tokio::test]
async fn the_memory_trait_store_does_not_wait_for_the_provider() {
    use crate::{Memory, MemoryCategory};

    let (_tmp, memory, release) = memory_with(false);

    // `slow` would block this call forever if `store` still embedded inline.
    memory
        .store(
            "test:deferred",
            "k",
            "slow body",
            MemoryCategory::Conversation,
            None,
        )
        .await
        .unwrap();
    release.add_permits(1);
}
