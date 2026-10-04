//! The agent-facing side of TinyMemory, over any
//! [`tinymemory_api::MemoryEngine`].
//!
//! - [`tools`] offers a model seven memory tools (recall, fetch, list, get,
//!   explore, store, forget) as runtime-neutral [`ToolSpec`]s, and runs them
//!   through [`MemoryTools::call`]. The namespace a model writes to and the
//!   reach it reads with are fixed by the host in a [`ToolScope`]; no tool
//!   argument can name either.
//! - [`context`] compiles `context.md`, a token-budgeted brief a host injects
//!   at the start of a session.
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//! use serde_json::json;
//! use tinymemory_api::conformance::ReferenceEngine;
//! use tinymemory_api::Namespace;
//! use tinymemory_tools::{MEMORY_RECALL, MEMORY_STORE, MemoryTools};
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread().build()?;
//! # runtime.block_on(async {
//! let engine = Arc::new(ReferenceEngine::new());
//! let tools = MemoryTools::new(engine).placed_at(Namespace::agent("researcher"));
//!
//! // Hand these to the tool runtime: name, description, JSON Schema.
//! let names: Vec<&str> = tools.specs().iter().map(|spec| spec.name).collect();
//! assert!(names.contains(&MEMORY_STORE));
//!
//! // Run what the model asked for.
//! tools
//!     .call(MEMORY_STORE, json!({
//!         "learning": { "text": "The user prefers short answers", "learning_kind": "preference" },
//!         "tags": ["style"]
//!     }))
//!     .await?;
//! let answer = tools
//!     .call(MEMORY_RECALL, json!({ "question": "How long should answers be?" }))
//!     .await?;
//! assert_eq!(answer["citations"][0]["meta"]["tags"], json!(["style"]));
//!
//! // Read-only tools neither list nor run the write tools.
//! let reader = tools.clone().read_only();
//! assert!(reader.specs().iter().all(|spec| spec.name != MEMORY_STORE));
//! assert!(reader.call(MEMORY_STORE, json!({})).await.is_err());
//! # Ok::<(), tinymemory_api::Error>(())
//! # })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

pub mod context;
pub mod recall;
pub mod tools;

pub use tools::{
    MEMORY_EXPLORE, MEMORY_FETCH, MEMORY_FORGET, MEMORY_GET, MEMORY_LIST, MEMORY_RECALL,
    MEMORY_STORE, MemoryTools, TOOL_NAMES, ToolScope, ToolSpec, WRITE_TOOL_NAMES,
};
