//! Ingestion over the hosted wire: where each kind of item lands, the key it
//! gets, the text the engine learns from, and what a resend costs.

#![allow(clippy::expect_used, clippy::panic)]

use tinymemory_api::chrono::{TimeZone, Utc};
use tinymemory_api::chunks::{DataSource, SourceKind, SourceRef};
use tinymemory_api::error::MemoryError;
use tinymemory_api::provider::{FastRetrieveQuery, MemoryIngest, MemoryRetrieval};
use tinymemory_api::types::{MemoryCategory, MemoryTaint};

use super::*;
use crate::cortex_provider::families::records::Version;
use crate::cortex_provider::families::test_support::{hosted, requests};

fn item(source: DataSource, source_id: &str, content: &str) -> IngestItem {
    IngestItem {
        namespace: None,
        source,
        source_id: source_id.to_string(),
        owner: "session-1".to_string(),
        source_ref: Some(SourceRef::new(format!("ref:{content}"))),
        content: content.to_string(),
        mime: Some("text/plain".to_string()),
        timestamp: Some(
            Utc.with_ymd_and_hms(2026, 9, 10, 9, 0, 0)
                .single()
                .expect("at"),
        ),
        tags: Vec::new(),
        author: Some("user".to_string()),
        channel_label: None,
        platform: Some("agent".to_string()),
        to: Vec::new(),
        cc: Vec::new(),
        subject: None,
        list_unsubscribe: None,
        taint: MemoryTaint::Internal,
        path_scope: None,
    }
}

fn chat(content: &str) -> IngestItem {
    item(DataSource::Conversation, "conversations:agent", content)
}

/// The live records of `namespace`, by key.
async fn live(provider: &CortexProvider, namespace: &str) -> Vec<Version> {
    let place = provider.ingest_place(namespace).expect("place");
    Records::new(&provider.dialect)
        .live_all(&place)
        .await
        .expect("records")
}

#[test]
fn mail_says_who_and_what_and_chat_does_not_repeat_a_role() {
    let mut mail = item(DataSource::Gmail, "thread-1", "See attached.");
    mail.author = Some("Ada <ada@example.com>".to_string());
    mail.to = vec!["bo@example.com".to_string()];
    mail.cc = vec!["cy@example.com".to_string(), "di@example.com".to_string()];
    mail.subject = Some("Invoice".to_string());
    mail.list_unsubscribe = Some("<mailto:u@example.com>".to_string());
    assert_eq!(
        item_text(&mail),
        "From: Ada <ada@example.com>\nTo: bo@example.com\nCc: cy@example.com, di@example.com\n\
         Subject: Invoice\nList-Unsubscribe: <mailto:u@example.com>\n\nSee attached."
    );
    assert_eq!(item_text(&chat("hello")), "hello");
}

#[tokio::test]
async fn messages_from_one_source_never_share_a_key() {
    let (provider, _state) = hosted().await;
    let first = provider
        .ingest_chat(vec![chat("first one"), chat("first two")])
        .await
        .expect("first batch");
    assert_eq!((first.written, first.skipped), (2, 0));
    assert_eq!(first.ids.len(), 2);
    provider
        .ingest_chat(vec![chat("second one"), chat("second two")])
        .await
        .expect("second batch");
    let records = live(&provider, "sources/chat").await;
    let mut contents: Vec<&str> = records
        .iter()
        .map(|version| version.record.content.as_str())
        .collect();
    contents.sort_unstable();
    assert_eq!(
        contents,
        ["first one", "first two", "second one", "second two"]
    );
    let message = &records[0].record;
    assert!(message.key.starts_with("message:conversations:agent:"));
    assert_eq!(message.category, MemoryCategory::Conversation);
    assert_eq!(message.session_id.as_deref(), Some("session-1"));
    assert_eq!(
        message.provenance.source.as_deref(),
        Some("conversations:agent")
    );
    assert!(message
        .provenance
        .reference
        .as_deref()
        .is_some_and(|reference| reference.starts_with("ref:")));
}

#[tokio::test]
async fn a_batch_resent_unchanged_writes_nothing() {
    let (provider, state) = hosted().await;
    let batch = vec![chat("one"), chat("two")];
    provider.ingest_chat(batch.clone()).await.expect("first");
    let writes = |state: &crate::hosted_test_support::Shared| {
        requests(state)
            .iter()
            .filter(|request| request.starts_with("POST /memory/experience"))
            .count()
    };
    let before = writes(&state);
    let again = provider.ingest_chat(batch).await.expect("again");
    assert_eq!((again.written, again.skipped), (0, 2));
    assert!(again.already_ingested);
    assert_eq!(again.ids.len(), 2);
    assert_eq!(writes(&state), before, "a resend writes nothing");
    assert!(provider
        .ingest_chat(Vec::new())
        .await
        .expect("empty")
        .ids
        .is_empty());
}

#[tokio::test]
async fn each_kind_lands_in_its_source_namespace_unless_it_names_one() {
    let (provider, _state) = hosted().await;
    let mut mail = item(DataSource::Gmail, "thread-1", "Lunch?");
    mail.author = Some("Ada".to_string());
    provider.ingest_email(vec![mail]).await.expect("mail");
    provider
        .ingest_document(item(DataSource::Notion, "page-1", "Plan"))
        .await
        .expect("document");
    let mut named = chat("kept apart");
    named.namespace = Some("team".to_string());
    provider.ingest_chat(vec![named]).await.expect("named");
    let email = live(&provider, source_namespace(SourceKind::Email)).await;
    assert_eq!(email.len(), 1);
    assert_eq!(email[0].record.content, "From: Ada\n\nLunch?");
    assert_eq!(email[0].record.session_id, None, "mail has no session");
    let documents = live(&provider, source_namespace(SourceKind::Document)).await;
    assert_eq!(documents[0].record.key, "document:page-1");
    assert_eq!(live(&provider, "team").await.len(), 1);
    assert!(live(&provider, "sources/chat").await.is_empty());
}

#[tokio::test]
async fn a_reingested_document_replaces_its_old_version() {
    let (provider, state) = hosted().await;
    provider
        .ingest_document(item(DataSource::Notion, "page-1", "Ship in May"))
        .await
        .expect("first");
    let again = provider
        .ingest_document(item(DataSource::Notion, "page-1", "Ship in May"))
        .await
        .expect("unchanged");
    assert_eq!((again.written, again.skipped), (0, 1));
    assert!(again.already_ingested);
    let changed = provider
        .ingest_document(item(DataSource::Notion, "page-1", "Ship in June"))
        .await
        .expect("changed");
    assert_eq!(changed.written, 1);
    let documents = live(&provider, "sources/documents").await;
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].record.content, "Ship in June");
    assert_eq!(
        state.log.lock().expect("log").forgotten.len(),
        1,
        "the replaced version is retired"
    );
}

#[tokio::test]
async fn ingested_content_is_retrieved_with_its_source() {
    let (provider, _state) = hosted().await;
    provider
        .ingest_chat(vec![chat("we chose oolong")])
        .await
        .expect("chat");
    let response = provider
        .fast_retrieve(
            "oolong",
            FastRetrieveQuery {
                limit: 5,
                max_hops: 1,
                time_window_days: None,
            },
            None,
        )
        .await
        .expect("retrieve");
    assert_eq!(response.hits.len(), 1);
    assert_eq!(response.hits[0].tree_scope, "conversations:agent");
    assert_eq!(response.hits[0].tree_kind.as_deref(), Some("chat"));
}

#[tokio::test]
async fn a_malformed_batch_is_refused_before_any_write() {
    let (provider, state) = hosted().await;
    let mut other = chat("b");
    other.source_id = "conversations:other".to_string();
    let mut elsewhere = chat("b");
    elsewhere.namespace = Some("team".to_string());
    let mut nameless = chat("a");
    nameless.source_id = " ".to_string();
    for batch in [
        vec![chat("a"), other],
        vec![chat("a"), elsewhere],
        vec![chat("a"), chat("  ")],
        vec![nameless],
    ] {
        let refused = provider.ingest_chat(batch).await;
        assert!(
            matches!(refused, Err(MemoryError::Invalid(_))),
            "{refused:?}"
        );
    }
    let refused = provider
        .ingest_document(item(DataSource::Notion, "page-1", " "))
        .await;
    assert!(
        matches!(refused, Err(MemoryError::Invalid(_))),
        "{refused:?}"
    );
    assert!(requests(&state).is_empty());
}
