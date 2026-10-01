//! Goals over the hosted wire.

#![allow(clippy::expect_used, clippy::panic)]

use tinymemory_api::error::MemoryError;
use tinymemory_api::goals::{GoalItem, GoalsDoc};
use tinymemory_api::provider::{MemoryCore, MemoryGoals};

use super::*;
use crate::cortex_provider::families::records::{Place, Record, Records};
use crate::cortex_provider::families::test_support::hosted;

fn doc(items: &[(&str, &str)]) -> GoalsDoc {
    GoalsDoc {
        items: items
            .iter()
            .map(|(id, text)| GoalItem {
                id: (*id).to_string(),
                text: (*text).to_string(),
            })
            .collect(),
    }
}

#[tokio::test]
async fn an_unwritten_document_reads_as_empty() {
    let (provider, _state) = hosted().await;
    assert_eq!(provider.goals().await.expect("goals"), GoalsDoc::default());
}

#[tokio::test]
async fn a_write_replaces_the_whole_document() {
    let (provider, state) = hosted().await;
    provider
        .set_goals(doc(&[("a", "learn rust"), ("b", "ship it")]))
        .await
        .expect("first");
    provider
        .set_goals(doc(&[("c", "rest")]))
        .await
        .expect("second");
    assert_eq!(
        provider.goals().await.expect("goals"),
        doc(&[("c", "rest")])
    );
    let held = state
        .log
        .lock()
        .expect("log")
        .events
        .iter()
        .filter(|e| e["scope"] == GOALS)
        .count();
    assert_eq!(held, 1, "the replaced version is retired");
}

#[tokio::test]
async fn an_unreadable_document_is_a_backend_error() {
    let (provider, _state) = hosted().await;
    let place = Place::bookkeeping(&provider.dialect, GOALS.to_string()).expect("place");
    Records::new(&provider.dialect)
        .put(&place, &Record::plain(GOALS_KEY, "not json"), None)
        .await
        .expect("put");
    assert!(matches!(
        provider.goals().await,
        Err(MemoryError::Backend(message)) if message.contains("unreadable goals")
    ));
}

#[tokio::test]
async fn the_document_is_none_of_the_users_namespaces() {
    let (provider, _state) = hosted().await;
    provider
        .set_goals(doc(&[("a", "learn rust")]))
        .await
        .expect("set");
    assert!(provider.namespaces().await.expect("namespaces").is_empty());
    assert!(provider
        .list(None, None, None)
        .await
        .expect("list")
        .is_empty());
}
