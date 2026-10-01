//! Bookkeeping scopes: where the families keep what is not the user's memory.
//!
//! Every bookkeeping scope has the segment type `tmi`. A namespace maps only to
//! `tm` and `tmx` segments, and the adapter's namespace listing drops any scope
//! with another type, so no bookkeeping record can surface in `namespaces`,
//! `list`, an export, or namespace recall.

/// The goals document.
pub(super) const GOALS: &str = "tmi:goals";

/// The learned profile's facets.
pub(super) const PROFILE: &str = "tmi:profile";

/// Episodic turns.
pub(super) const TURNS: &str = "tmi:turns";

/// Conversation segments.
pub(super) const SEGMENTS: &str = "tmi:segments";

/// Episodic events.
pub(super) const EPISODIC_EVENTS: &str = "tmi:episodic-events";

/// Segment embeddings.
pub(super) const SEGMENT_EMBEDDINGS: &str = "tmi:segment-embeddings";

/// The parent of every document-details scope.
const DOCUMENT_DETAILS: &str = "tmi:documents";

/// The scope holding the details of the documents in the namespace whose scope
/// is `namespace_scope`. It is one segment deeper than that scope.
pub(super) fn document_details(namespace_scope: &str) -> String {
    format!("{DOCUMENT_DETAILS}/{namespace_scope}")
}

#[cfg(test)]
#[path = "scopes_test.rs"]
mod test;
