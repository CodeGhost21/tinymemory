//! The agent-facing side of TinyMemory, over any
//! [`tinymemory_api::MemoryEngine`].
//!
//! - [`context`] compiles `context.md`, a token-budgeted brief a host injects
//!   at the start of a session.

pub mod context;
