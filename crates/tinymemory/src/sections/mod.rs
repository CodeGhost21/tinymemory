//! Typed surfaces for the sections the namespace convention names:
//! conversations, learnings, and documents — plus a recall that can span one.
//!
//! Namespaces are the contract's only partitioning primitive and they cross it
//! as a bare `&str`. `MemorySection` gives the string a shape —
//! `<section>:<scope>`, so `conversation:thread-8f21` and `learning:rust-async`
//! mean the same thing to every host and every engine — but nothing made using
//! it easier than concatenating the prefix by hand, where a typo produces a
//! valid, silently wrong namespace instead of an error.
//!
//! This module is that missing ergonomics layer, and nothing more:
//!
//! ```text
//! Sections::new(provider)
//!   ├── conversations()  ─┐
//!   ├── learnings()       ├─ SectionView  put / get / forget / list
//!   ├── documents()       │                scopes / list_section
//!   ├── section(custom)  ─┘
//!   └── recall()          ── SectionRecall  in_scope / across_section
//! ```
//!
//! ## Mandatory families only
//!
//! Every call here composes `MemoryCore` and `MemoryRecall`, which every driver
//! implements as supertraits. So the whole surface works on *any* provider —
//! there is no capability to negotiate, no accessor that can return `None`, and
//! no "unsupported" path to handle. On a driver that retains nothing, every call
//! succeeds and returns empty.
//!
//! ## This is not the document intake path
//!
//! [`Sections::documents`] writes through `MemoryCore`, so it is for text you
//! already hold. Handing the memory layer a *file* — sniffing its format,
//! converting it to markdown, then choosing between `MemoryIngest`,
//! `MemoryDocuments` and `MemoryCore` — is `DocumentIntake`'s job in the
//! `documents` module, and it is the right entry point for an upload. Reaching
//! for this one instead would build a second, worse intake.
//!
//! ## Borrowed, not owned
//!
//! Every handle holds a `&dyn MemoryProvider`. They are cheap to create and
//! discard, hold no state between calls, and cannot outlive the provider — so a
//! caller makes one where it is needed rather than threading it through a
//! struct.
//!
//! ## Example
//!
//! ```
//! use std::sync::Arc;
//!
//! use tinymemory::namespace::MemorySection;
//! use tinymemory::provider::MemoryProvider;
//! use tinymemory::sections::Sections;
//! use tinymemory::types::{MemoryCategory, MemoryTaint};
//! use tinymemory_conformance::InMemoryProvider;
//!
//! let provider: Arc<dyn MemoryProvider> = Arc::new(InMemoryProvider::new());
//! let runtime = tokio::runtime::Runtime::new()?;
//!
//! runtime.block_on(async {
//!     let sections = Sections::new(provider.as_ref());
//!
//!     // The caller names a scope; the handle owns the prefix.
//!     let namespace = sections
//!         .conversations()
//!         .put(
//!             "thread-8f21",
//!             "turn-1",
//!             "we agreed to ship on the 14th",
//!             MemoryCategory::Core,
//!             None,
//!             MemoryTaint::Internal,
//!         )
//!         .await?;
//!     assert_eq!(namespace.as_str(), "conversation:thread-8f21");
//!
//!     // Discover the scopes a section holds, without knowing the convention.
//!     let scopes = sections.conversations().scopes().await?;
//!     assert_eq!(scopes.len(), 1);
//!     assert_eq!(scopes[0].scope(), "thread-8f21");
//!
//!     // Ask the whole section one question.
//!     let found = sections
//!         .recall()
//!         .across_section(&MemorySection::Conversation, "ship", 10, &Default::default(), None)
//!         .await?;
//!     assert_eq!(found.namespaces_searched, 1);
//!     assert!(!found.truncated);
//!
//!     Ok::<(), tinymemory::error::MemoryError>(())
//! })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

use std::fmt;

use tinymemory_api::namespace::MemorySection;
use tinymemory_api::provider::MemoryProvider;

// Private, with the whole surface re-exported below: one public path per item,
// as `registry` does with its own submodules.
mod recall;
mod types;
mod view;

pub use recall::SectionRecall;
pub use types::{
    SectionHits, SectionScope, CROSS_SESSION_FAN_OUT_CONFLICT, CROSS_SESSION_SECTION_CONFLICT,
    MAX_SECTION_NAMESPACES, NAMESPACE_FILTER_CONFLICT,
};
pub use view::SectionView;

/// The entry point: a provider, viewed one section at a time.
///
/// A borrowing handle, so build one where you need it rather than storing it.
#[derive(Clone, Copy)]
pub struct Sections<'a> {
    provider: &'a dyn MemoryProvider,
}

impl fmt::Debug for Sections<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sections")
            .field("driver_id", &self.provider.driver_id())
            .finish()
    }
}

impl<'a> Sections<'a> {
    /// Bind `provider`.
    #[must_use]
    pub fn new(provider: &'a dyn MemoryProvider) -> Self {
        Self { provider }
    }

    /// Turn-by-turn conversational memory — the `conversation:` section.
    #[must_use]
    pub fn conversations(&self) -> SectionView<'a> {
        self.section(&MemorySection::Conversation)
    }

    /// Durable conclusions the agent drew and expects to reuse — the
    /// `learning:` section.
    #[must_use]
    pub fn learnings(&self) -> SectionView<'a> {
        self.section(&MemorySection::Learning)
    }

    /// Whole documents and the collections they sit in — the `document:`
    /// section, informally "the brain".
    ///
    /// For text you already hold. An upload belongs to `DocumentIntake`; see
    /// the module docs.
    #[must_use]
    pub fn documents(&self) -> SectionView<'a> {
        self.section(&MemorySection::Document)
    }

    /// Any section, including the four this type has no named accessor for
    /// (`entity:`, `profile:`, `tool:`, `source:`) and
    /// [`MemorySection::Custom`].
    ///
    /// The named accessors are the three sections a host writes to routinely;
    /// this is the same view over the rest of the vocabulary, so nothing is
    /// second-class.
    ///
    /// Taken by reference, as every section argument in this module is, so a
    /// caller never has to remember which side wants which.
    #[must_use]
    pub fn section(&self, section: &MemorySection) -> SectionView<'a> {
        SectionView::new(self.provider, section)
    }

    /// Section-aware recall.
    #[must_use]
    pub fn recall(&self) -> SectionRecall<'a> {
        SectionRecall::new(self.provider)
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod test;
