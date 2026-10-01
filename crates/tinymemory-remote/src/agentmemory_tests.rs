//! AgentMemory adapter tests over its native REST routes.

#![allow(clippy::expect_used)]

use std::sync::{Arc, Mutex};

use axum::{
    extract::State,
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use tinymemory_api::{
    provider::{MemoryCore, MemoryProvider, MemoryRecall},
    recall::OwnedRecallOpts,
    types::{MemoryCategory, MemoryTaint},
};

#[derive(Clone, Default)]
struct AppState(Arc<Mutex<Vec<Value>>>);

async fn livez() -> Json<Value> {
    Json(json!({"status": "ok", "service": "agentmemory"}))
}

async fn memories(State(state): State<AppState>) -> Json<Value> {
    Json(
        json!({"memories": state.0.lock().expect("state lock").clone(), "total": state.0.lock().expect("state lock").len()}),
    )
}

async fn remember(State(state): State<AppState>, Json(body): Json<Value>) -> Json<Value> {
    let mut memories = state.0.lock().expect("state lock");
    let id = format!("mem_{}", memories.len() + 1);
    let content = body["content"].as_str().expect("content").to_owned();
    let memory = json!({"id": id, "content": content, "createdAt": "2026-08-23T00:00:00Z", "updatedAt": "2026-08-23T00:00:00Z"});
    memories.push(memory.clone());
    Json(json!({"success": true, "memory": memory}))
}

async fn forget(State(state): State<AppState>, Json(body): Json<Value>) -> Json<Value> {
    let id = body["memoryId"].as_str().expect("memory id");
    state
        .0
        .lock()
        .expect("state lock")
        .retain(|memory| memory["id"] != id);
    Json(json!({"deleted": 1}))
}

async fn search(State(state): State<AppState>, Json(body): Json<Value>) -> Json<Value> {
    let query = body["query"].as_str().expect("query").to_ascii_lowercase();
    let results: Vec<Value> = state.0.lock().expect("state lock").iter().filter(|memory| memory["content"].as_str().is_some_and(|content| content.to_ascii_lowercase().contains(&query))).map(|memory| json!({"observation": {"id": memory["id"], "narrative": memory["content"]}, "score": 0.9, "sessionId": "memory"})).collect();
    Json(json!({"format": "full", "results": results}))
}

async fn driver() -> crate::AgentMemoryMemory {
    let app = Router::new()
        .route("/agentmemory/livez", get(livez))
        .route("/agentmemory/memories", get(memories))
        .route("/agentmemory/remember", post(remember))
        .route("/agentmemory/forget", post(forget))
        .route("/agentmemory/search", post(search))
        .with_state(AppState::default());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let endpoint = format!("http://{}", listener.local_addr().expect("address"));
    tokio::spawn(async move {
        axum::serve(listener, app).await.expect("serve");
    });
    super::AgentMemoryMemory::new(&endpoint, None).expect("client")
}

#[tokio::test]
async fn native_agentmemory_routes_preserve_tinymemory_records() {
    let driver = crate::agentmemory_provider(driver().await);
    tinymemory_api::provider::audit_provider(&driver).expect("honest capabilities");
    assert!(driver.health().await.is_usable());
    driver
        .store(
            "people",
            "alice",
            "likes tea",
            MemoryCategory::Core,
            Some("s1"),
            MemoryTaint::ExternalSync,
        )
        .await
        .expect("store");
    let stored = driver
        .get("people", "alice")
        .await
        .expect("get")
        .expect("stored");
    assert_eq!(stored.content, "likes tea");
    assert_eq!(stored.taint, MemoryTaint::ExternalSync);
    driver
        .store(
            "people",
            "alice",
            "likes coffee",
            MemoryCategory::Daily,
            Some("s2"),
            MemoryTaint::Internal,
        )
        .await
        .expect("upsert");
    assert_eq!(driver.list(None, None, None).await.expect("list").len(), 1);
    let hits = driver
        .recall(
            "coffee",
            10,
            &OwnedRecallOpts {
                namespace: Some("people".into()),
                ..OwnedRecallOpts::default()
            },
            None,
        )
        .await
        .expect("recall");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].content, "likes coffee");
    assert!(driver.forget("people", "alice").await.expect("forget"));
    assert!(driver
        .get("people", "alice")
        .await
        .expect("get after forget")
        .is_none());
}

#[test]
fn agentmemory_driver_id_is_stable() {
    assert_eq!(super::AGENTMEMORY_DRIVER_ID, "agentmemory");
    assert_eq!(super::AGENTMEMORY_API_ENDPOINT, "http://localhost:3111");
}
