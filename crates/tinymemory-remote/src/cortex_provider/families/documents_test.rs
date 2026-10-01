//! Documents over the hosted wire.

#![allow(clippy::expect_used, clippy::panic)]

use serde_json::json;
use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::{MemoryCore, MemoryDocuments};
use tinymemory_api::types::{MemoryCategory, MemoryTaint, NamespaceDocumentInput};

use crate::cortex_provider::families::test_support::hosted;

fn input(namespace: &str, key: &str, title: &str, content: &str) -> NamespaceDocumentInput {
    NamespaceDocumentInput {
        namespace: namespace.to_string(),
        key: key.to_string(),
        title: title.to_string(),
        content: content.to_string(),
        source_type: "note".to_string(),
        priority: "high".to_string(),
        tags: vec!["tag".to_string()],
        metadata: json!({ "lang": "en" }),
        category: "core".to_string(),
        session_id: None,
        document_id: None,
        taint: MemoryTaint::Internal,
    }
}

#[tokio::test]
async fn documents_pass_the_conformance_round_trip() {
    let (provider, _state) = hosted().await;
    tinymemory_conformance::suite::assert_documents_round_trip(&provider).await;
}

#[tokio::test]
async fn a_document_keeps_its_details_and_is_the_keys_record() {
    let (provider, _state) = hosted().await;
    let id = provider
        .put_document(input("notes", "tea", "Tea", "oolong, not too hot"))
        .await
        .expect("put");
    let stored = provider
        .get_document("notes", "tea")
        .await
        .expect("get")
        .expect("document");
    assert_eq!(stored.document_id, id);
    assert_eq!(stored.title, "Tea");
    assert_eq!(stored.source_type, "note");
    assert_eq!(stored.priority, "high");
    assert_eq!(stored.tags, ["tag"]);
    assert_eq!(stored.metadata, json!({ "lang": "en" }));
    assert!(stored.created_at > 0.0);
    // The body is the namespace's own record under the document's key.
    let entry = provider
        .get("notes", "tea")
        .await
        .expect("get")
        .expect("entry");
    assert_eq!(entry.content, "oolong, not too hot");
}

#[tokio::test]
async fn a_document_keeps_its_id_and_creation_time_across_rewrites() {
    let (provider, _state) = hosted().await;
    let first = provider
        .put_document(NamespaceDocumentInput {
            document_id: Some("chosen-id".to_string()),
            ..input("notes", "tea", "Tea", "one")
        })
        .await
        .expect("put");
    assert_eq!(first, "chosen-id");
    let created = provider
        .get_document("notes", "tea")
        .await
        .expect("get")
        .expect("document")
        .created_at;
    let second = provider
        .put_document(NamespaceDocumentInput {
            document_id: Some("another-id".to_string()),
            ..input("notes", "tea", "Tea", "two")
        })
        .await
        .expect("rewrite");
    assert_eq!(second, "chosen-id", "an existing document keeps its id");
    let stored = provider
        .get_document("notes", "tea")
        .await
        .expect("get")
        .expect("document");
    assert_eq!(stored.content, "two");
    assert_eq!(stored.created_at, created);
    assert!(stored.updated_at >= created);
}

#[tokio::test]
async fn a_plain_store_over_a_document_reads_with_default_details() {
    let (provider, _state) = hosted().await;
    provider
        .put_document(NamespaceDocumentInput {
            document_id: Some("chosen-id".to_string()),
            ..input("notes", "tea", "Tea", "document body")
        })
        .await
        .expect("put");
    provider
        .store(
            "notes",
            "tea",
            "plain body",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    let stored = provider
        .get_document("notes", "tea")
        .await
        .expect("get")
        .expect("still a document");
    assert_eq!(stored.content, "plain body");
    assert_eq!(stored.title, "tea", "the old details no longer apply");
    assert_eq!(stored.source_type, "chat");
    assert_ne!(stored.document_id, "chosen-id");
    let listed = provider.list_documents(Some("notes")).await.expect("list");
    assert_eq!(listed["count"], json!(1));
}

#[tokio::test]
async fn a_record_store_wrote_is_a_document_as_in_the_embedded_engine() {
    let (provider, _state) = hosted().await;
    provider
        .store(
            "notes",
            "note",
            "a plain note",
            MemoryCategory::Core,
            None,
            MemoryTaint::Internal,
        )
        .await
        .expect("store");
    let stored = provider
        .get_document("notes", "note")
        .await
        .expect("get")
        .expect("document");
    assert_eq!(stored.title, "note");
    assert_eq!(stored.source_type, "chat");
    assert_eq!(stored.priority, "medium");
    assert_eq!(stored.metadata, json!({}));
    assert!(stored.updated_at > 0.0, "the engine's recorded time");
    assert_eq!(
        provider.list_namespaces().await.expect("namespaces"),
        ["notes"]
    );
    let removed = provider
        .delete_document("notes", &stored.document_id)
        .await
        .expect("delete");
    assert_eq!(removed["deleted"], json!(true));
    assert!(provider.get("notes", "note").await.expect("get").is_none());
}

#[tokio::test]
async fn synced_items_are_not_documents() {
    use tinymemory_api::provider::types::SourceItem;
    use tinymemory_api::provider::MemorySourceSink;
    let (provider, _state) = hosted().await;
    provider
        .accept_source_items(
            "notion:ws",
            "composio",
            vec![SourceItem {
                item_id: "1".to_string(),
                title: String::new(),
                content: "a synced page".to_string(),
                mime: None,
                url: None,
                updated_at_ms: None,
                tags: Vec::new(),
            }],
            MemoryTaint::ExternalSync,
        )
        .await
        .expect("accept");
    assert!(provider
        .list_namespaces()
        .await
        .expect("namespaces")
        .is_empty());
    let listed = provider.list_documents(None).await.expect("list");
    assert_eq!(listed["count"], json!(0));
}

#[tokio::test]
async fn a_listing_is_newest_first_across_namespaces() {
    let (provider, _state) = hosted().await;
    for (namespace, key) in [("notes", "a"), ("work", "b"), ("notes", "c")] {
        provider
            .put_document(input(namespace, key, key, "body"))
            .await
            .expect("put");
    }
    let listed = provider.list_documents(None).await.expect("list");
    assert_eq!(listed["count"], json!(3));
    let keys: Vec<&str> = listed["documents"]
        .as_array()
        .expect("rows")
        .iter()
        .map(|row| row["key"].as_str().expect("key"))
        .collect();
    assert_eq!(keys, ["c", "b", "a"]);
    let namespaces = provider.list_namespaces().await.expect("namespaces");
    assert_eq!(namespaces, ["notes", "work"]);
}

#[tokio::test]
async fn a_namespace_whose_documents_are_all_gone_is_not_listed() {
    let (provider, _state) = hosted().await;
    let id = provider
        .put_document(input("notes", "a", "A", "body"))
        .await
        .expect("put");
    let removed = provider
        .delete_document("notes", &id)
        .await
        .expect("delete");
    assert_eq!(
        removed,
        json!({ "deleted": true, "namespace": "notes", "documentId": id })
    );
    assert!(provider
        .list_namespaces()
        .await
        .expect("namespaces")
        .is_empty());
    let missing = provider
        .delete_document("notes", "no-such-id")
        .await
        .expect("delete");
    assert_eq!(missing["deleted"], json!(false));
}

#[tokio::test]
async fn clearing_a_namespace_takes_its_details_with_it() {
    let (provider, state) = hosted().await;
    provider
        .put_document(input("notes", "a", "A", "body"))
        .await
        .expect("put");
    provider.clear_namespace("notes").await.expect("clear");
    assert!(provider
        .get_document("notes", "a")
        .await
        .expect("get")
        .is_none());
    let left = state.log.lock().expect("log").events.len();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn a_query_ranks_the_engines_hits_and_renders_their_context() {
    let (provider, _state) = hosted().await;
    for (key, title, body) in [
        ("tea", "Tea", "The user drinks oolong tea."),
        ("rent", "Rent", "Rent is due on the fifth."),
    ] {
        provider
            .put_document(input("notes", key, title, body))
            .await
            .expect("put");
    }
    // The double's recall matches by substring.
    let context = provider
        .query_documents("notes", "oolong", 5)
        .await
        .expect("query");
    assert_eq!(context.hits.len(), 1);
    let hit = &context.hits[0];
    assert_eq!(hit.key, "tea");
    assert_eq!(hit.title.as_deref(), Some("Tea"));
    assert!(hit.document_id.is_some());
    assert!(hit.score > 0.9, "{hit:?}");
    assert_eq!(hit.score_breakdown.keyword_relevance, 1.0);
    assert_eq!(hit.score_breakdown.final_score, hit.score);
    assert_eq!(
        context.context_text,
        "Query: oolong\n\nTea: The user drinks oolong tea."
    );
    let nothing = provider
        .query_documents("notes", "zebra", 5)
        .await
        .expect("query");
    assert!(nothing.hits.is_empty());
}

#[tokio::test]
async fn recall_without_a_query_returns_the_newest_documents() {
    let (provider, _state) = hosted().await;
    for key in ["old", "new"] {
        provider
            .put_document(input("notes", key, key, "body"))
            .await
            .expect("put");
    }
    let recalled = provider.recall_documents("notes", 1).await.expect("recall");
    assert_eq!(recalled.hits.len(), 1);
    assert_eq!(recalled.hits[0].key, "new");
    assert!(recalled.hits[0].score > 0.9);
    assert_eq!(recalled.context_text, "new: body");
}

#[tokio::test]
async fn a_document_needs_a_namespace_and_a_key() {
    let (provider, _state) = hosted().await;
    for bad in [input(" ", "k", "t", "c"), input("notes", "", "t", "c")] {
        assert!(matches!(
            provider.put_document(bad).await,
            Err(MemoryError::Invalid(_))
        ));
    }
}

#[tokio::test]
async fn content_that_lost_its_details_reads_with_defaults() {
    let (provider, state) = hosted().await;
    provider
        .put_document(input("notes", "tea", "Tea", "body"))
        .await
        .expect("put");
    // Drop the details record, as a failed second write would leave it.
    state
        .log
        .lock()
        .expect("log")
        .events
        .retain(|event| !event["scope"].as_str().expect("scope").starts_with("tmi:"));
    let stored = provider
        .get_document("notes", "tea")
        .await
        .expect("get")
        .expect("document");
    assert_eq!(stored.title, "tea");
    assert_eq!(stored.content, "body");
}
