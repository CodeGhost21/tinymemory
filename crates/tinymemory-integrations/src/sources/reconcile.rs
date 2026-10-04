//! The pure half of Composio reconciliation: what a scanned connection looks
//! like as a registry row, and which caps a cap-less row gets on migration.
//!
//! Scanning live connections and persisting the registry are the host's (they
//! need its credentials, config file and write lock); the functions here are
//! the decisions, with no I/O, so they are unit-tested directly.

use crate::sources::registry::{
    apply_kind_defaults, memory_sync_defaults_for_toolkit, ComposioUpsertTarget,
};
use crate::sources::types::{MemorySourceEntry, SourceKind};

/// Build the `(toolkit, connection_id, label)` upsert target for one scanned
/// Composio connection.
///
/// The label is a title-cased toolkit name plus the truncated connection id so
/// distinct accounts of the same toolkit (e.g. two Gmail logins) don't all show
/// as "Gmail connection".
pub fn composio_upsert_target(toolkit: &str, connection_id: &str) -> ComposioUpsertTarget {
    let label = format!("{} · {}", title_case(toolkit), short_id(connection_id));
    (toolkit.to_string(), connection_id.to_string(), label)
}

fn title_case(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(c) => c.to_uppercase().chain(chars).collect(),
    }
}

fn short_id(id: &str) -> &str {
    // Show only the last 8 Unicode scalar values to keep labels compact.
    // Byte-slicing would panic if the cut point isn't a UTF-8 boundary.
    let n = id.chars().count();
    if n <= 8 {
        return id;
    }
    let skip = n - 8;
    let start = id.char_indices().nth(skip).map(|(idx, _)| idx).unwrap_or(0);
    &id[start..]
}

/// Apply conservative default caps in place to every cap-less source.
///
/// For a Composio source with no `max_items` / `sync_depth_days`, writes the
/// per-toolkit defaults **and enables it** (a no-op when already enabled) — an
/// already-enabled, cap-less source would otherwise sync at the provider's
/// large internal ceiling instead of the cheap default, which is the cost this
/// migration exists to avoid. For other kinds it fills any unset kind-specific
/// caps through [`apply_kind_defaults`]. Caps the user has
/// customised (any non-`None` value) are never overwritten.
///
/// Returns the number of Composio entries that received defaults. Pure (no
/// I/O) so it can be unit-tested directly.
pub fn apply_caps_defaults_to_entries(sources: &mut [MemorySourceEntry]) -> u32 {
    let mut applied = 0u32;
    for source in sources.iter_mut() {
        match source.kind {
            SourceKind::Composio => {
                // Applies to enabled AND disabled cap-less sources; skips
                // entries the user has already customised (any non-None cap).
                if source.max_items.is_none() && source.sync_depth_days.is_none() {
                    let toolkit = source.toolkit.as_deref().unwrap_or("");
                    let (max_items, sync_depth_days) = memory_sync_defaults_for_toolkit(toolkit);
                    log::debug!(
                        "[memory_sources:reconcile] caps migration: applying conservative defaults \
                         id={} toolkit={toolkit} was_enabled={} max_items={max_items:?} \
                         sync_depth_days={sync_depth_days:?}",
                        source.id,
                        source.enabled
                    );
                    source.enabled = true;
                    source.max_items = max_items;
                    source.sync_depth_days = sync_depth_days;
                    applied += 1;
                }
            }
            // Non-composio kinds get their kind defaults through the same
            // helper the CRUD path uses, so one table of conservative values
            // serves both.
            _ => apply_kind_defaults(source),
        }
    }
    applied
}

#[cfg(test)]
#[path = "reconcile_tests.rs"]
mod tests;
