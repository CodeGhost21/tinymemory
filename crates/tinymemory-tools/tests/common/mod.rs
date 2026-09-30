//! A test host over the conformance crate's recording provider.

#![allow(dead_code)]

use std::sync::Arc;

use async_trait::async_trait;
use tinymemory_api::provider::MemoryProvider;
use tinymemory_conformance::RecordingProvider;
use tinymemory_tools::{MemoryToolHost, QueryEmbedder};
use tinytools::ToolResult;

/// A host whose provider is a [`RecordingProvider`] (or nothing at all).
#[derive(Clone)]
pub struct TestHost {
    pub provider: Option<Arc<RecordingProvider>>,
    pub chunks_allowed: bool,
}

impl TestHost {
    /// A host bound to a fresh recording provider.
    pub fn bound() -> Self {
        Self {
            provider: Some(Arc::new(RecordingProvider::new())),
            chunks_allowed: true,
        }
    }

    /// A host with no memory bound: every `provider()` call fails.
    pub fn unbound() -> Self {
        Self {
            provider: None,
            chunks_allowed: true,
        }
    }

    /// The recorded driver calls, as their method names.
    pub fn methods(&self) -> Vec<String> {
        self.provider
            .as_ref()
            .map(|p| p.calls().into_iter().map(|c| c.method).collect())
            .unwrap_or_default()
    }

    /// Whether every recorded call was handed no explicit scope (the guard
    /// applies the ambient one).
    pub fn all_unscoped(&self) -> bool {
        self.provider
            .as_ref()
            .map(|p| p.calls().iter().all(|c| c.scoped != Some(true)))
            .unwrap_or(true)
    }
}

struct FixedEmbedder;

#[async_trait]
impl QueryEmbedder for FixedEmbedder {
    async fn embed_one(&self, _text: &str) -> Result<Vec<f32>, String> {
        Ok(vec![1.0, 0.0])
    }
    fn signature(&self) -> String {
        "provider=test;model=fixed;dims=2".to_string()
    }
}

#[async_trait]
impl MemoryToolHost for TestHost {
    async fn provider(&self) -> Result<Arc<dyn MemoryProvider>, String> {
        match &self.provider {
            Some(p) => Ok(p.clone() as Arc<dyn MemoryProvider>),
            None => Err("no memory driver is bound".to_string()),
        }
    }

    fn chunk_source_allowed(&self, _tags: &[String], _source_id: &str) -> bool {
        self.chunks_allowed
    }

    async fn embedder(&self) -> Result<Box<dyn QueryEmbedder>, String> {
        if self.provider.is_some() {
            Ok(Box::new(FixedEmbedder))
        } else {
            Err("load config failed: no workspace".to_string())
        }
    }

    async fn ingest_document(&self, args: serde_json::Value) -> anyhow::Result<ToolResult> {
        Ok(ToolResult::success(format!("host-ingest:{args}")))
    }
}
