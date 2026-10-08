//! Lookup labels: fixed-length digests a label filter can find events by.
//!
//! Every event carries `tm:i:<digest(item id)>`, so all of an item's events
//! (a conversation's turns, or a re-write) are found with one label filter.
//! It also carries one label per exact-match metadata field CortexDB can then
//! narrow by server-side ([`META_FIELDS`]).
//!
//! A label holds a digest of the value, not the value: the engine splits its
//! label filter on commas and bounds a label's length, and a path or a source
//! id may be long or hold a comma. A label only ever narrows; every reader
//! re-applies the full [`MetaFilter`] to the envelope it gets back, so a
//! digest collision costs a wasted row, never a wrong answer.

use sha2::{Digest, Sha256};
use tinymemory_api::{MemoryMeta, MetaFilter};

/// Hex digits of the SHA-256 a label keeps: 64 bits.
const DIGEST_CHARS: usize = 16;

/// Hex digits of the SHA-256 a path digest keeps: 160 bits, as many as an
/// item id ([`tinymemory_api::StoreItem::fingerprint`]).
const PATH_DIGEST_CHARS: usize = 40;

/// The first [`DIGEST_CHARS`] lowercase hex digits of `value`'s SHA-256.
pub(crate) fn digest(value: &str) -> String {
    hex_prefix(value, DIGEST_CHARS)
}

/// The first [`PATH_DIGEST_CHARS`] lowercase hex digits of `value`'s
/// SHA-256: the digest an envelope keeps in place of a local path, which a
/// filter is matched against with nothing left to re-check, so it is as
/// strong as an item id. Its first [`DIGEST_CHARS`] are [`digest`].
pub(crate) fn path_digest(value: &str) -> String {
    hex_prefix(value, PATH_DIGEST_CHARS)
}

/// The first `chars` lowercase hex digits of `value`'s SHA-256.
fn hex_prefix(value: &str, chars: usize) -> String {
    let mut out = String::with_capacity(chars + 2);
    for byte in Sha256::digest(value.as_bytes()) {
        if out.len() >= chars {
            break;
        }
        out.push_str(&format!("{byte:02x}"));
    }
    out.truncate(chars);
    out
}

/// Prefix of the workspace label.
const WORKSPACE_PREFIX: &str = "tm:w:";

/// The workspace label for a workspace whose [`path_digest`] is `digest`:
/// an envelope keeps only that, and a label's [`digest`] is its prefix.
pub(crate) fn workspace(digest: &str) -> String {
    let short = digest.get(..DIGEST_CHARS).unwrap_or(digest);
    format!("{WORKSPACE_PREFIX}{short}")
}

/// The label every event of item `id` carries.
pub(crate) fn item(id: &str) -> String {
    format!("tm:i:{}", digest(id))
}

/// One labelled metadata field: its label prefix, how to read it from
/// stored metadata, and how to read the wanted value from a filter.
struct MetaField {
    prefix: &'static str,
    held: fn(&MemoryMeta) -> Option<&str>,
    wanted: fn(&MetaFilter) -> Option<&str>,
}

/// The exact-match fields that are labelled, in the order a filter picks one
/// to narrow by: the most selective first. `folder` and `file_path` are not
/// here because they also match as a prefix, which a digest cannot.
const META_FIELDS: [MetaField; 6] = [
    MetaField {
        prefix: "tm:t:",
        held: |m| m.thread_id.as_deref(),
        wanted: |f| f.thread_id.as_deref(),
    },
    MetaField {
        prefix: "tm:s:",
        held: |m| m.source.id.as_deref(),
        wanted: |f| f.source_id.as_deref(),
    },
    MetaField {
        prefix: "tm:r:",
        held: |m| m.repo.as_deref(),
        wanted: |f| f.repo.as_deref(),
    },
    MetaField {
        prefix: WORKSPACE_PREFIX,
        held: |m| m.workspace.as_deref(),
        wanted: |f| f.workspace.as_deref(),
    },
    MetaField {
        prefix: "tm:a:",
        held: |m| m.agent_id.as_deref(),
        wanted: |f| f.agent_id.as_deref(),
    },
    MetaField {
        prefix: "tm:l:",
        held: |m| m.language.as_deref(),
        wanted: |f| f.language.as_deref(),
    },
];

/// Prefix of the source-kind label. Unlike the fields above a filter may ask
/// for several source kinds, which a label filter's any-of reading serves.
const SOURCE_KIND_PREFIX: &str = "tm:k:";

/// Every label an event of item `id` with `meta` carries: the item label, one
/// per set labelled field, and the source kind. At most eight.
pub(crate) fn for_item(id: &str, meta: &MemoryMeta) -> Vec<String> {
    let mut labels = vec![item(id)];
    for field in &META_FIELDS {
        if let Some(value) = (field.held)(meta) {
            labels.push(format!("{}{}", field.prefix, digest(value)));
        }
    }
    labels.push(format!(
        "{SOURCE_KIND_PREFIX}{}",
        digest(meta.source.kind.as_str())
    ));
    labels
}

/// The one label filter that narrows a read for `filter`, if any field of it
/// is labelled: the first set field of [`META_FIELDS`], else the source
/// kinds. The engine keeps events carrying any one of the returned labels,
/// so only labels of one field may be sent together. A wanted id holding a
/// phone number is sent both as itself and as its redacted form (see the
/// envelope's `stored_id`), so events stored before and after redaction are
/// both found.
pub(crate) fn narrowing(filter: &MetaFilter) -> Option<Vec<String>> {
    for field in &META_FIELDS {
        if let Some(value) = (field.wanted)(filter) {
            let mut labels = vec![format!("{}{}", field.prefix, digest(value))];
            if tinymemory_api::holds_phone_number(value) {
                let redacted = tinymemory_api::redacted_id(value);
                labels.push(format!("{}{}", field.prefix, digest(&redacted)));
            }
            return Some(labels);
        }
    }
    (!filter.sources.is_empty()).then(|| {
        filter
            .sources
            .iter()
            .map(|kind| format!("{SOURCE_KIND_PREFIX}{}", digest(kind.as_str())))
            .collect()
    })
}

#[cfg(test)]
#[path = "labels_tests.rs"]
mod tests;
