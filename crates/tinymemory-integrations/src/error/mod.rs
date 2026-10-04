//! The crate-wide error is the contract's error.
//!
//! Every integration ends at a [`tinymemory_api::MemoryEngine`] call or
//! produces a [`tinymemory_api::StoreItem`] for one, so the error a host acts
//! on is [`tinymemory_api::Error`]. The CortexDB engine and the registry return
//! it directly. `documents`, `sources` and `import` keep a typed error of their
//! own, because their failures (a path escaping its root, a non-v1 workspace)
//! are worth matching on before they reach an engine, and each converts into
//! this one with `From`/`?`.

pub use tinymemory_api::Error;

/// The crate-wide result alias.
pub type Result<T> = std::result::Result<T, Error>;
