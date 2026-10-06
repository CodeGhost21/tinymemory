//! What a recall pack says about itself, surfaced in the host's log.
//!
//! A pack can come back complete and still say it is not what was asked
//! for. Nothing here changes a read; it makes these visible:
//!
//! - **`warnings[]`.** Every entry is logged at debug, with the scope.
//!   `parent_pack_unranked_sample: …` (CortexDB 0.10.4) is logged at warn:
//!   it marks a pack at a parent scope whose child events were read in
//!   storage order, not ranked. This crate reads one exact scope per pack
//!   (`view: "granular"`), so it must never appear; if it does, a read
//!   strayed into a parent scope.
//! - **Knapsack evictions.** `budgets.max_tokens` (4000 by default) is a
//!   cross-layer budget; items it evicts are counted in
//!   `diagnostics.knapsack_evictions` (when the caller may read
//!   diagnostics), and a rendered event evicted from `layers.events` is named
//!   by the `context_contributors` provenance entry with
//!   `evicted_from_layers: true`. Either is logged at warn with the scope,
//!   since an evicted event is a hit the engine never sees. A granular pack
//!   renders no `context_block`, so it carries no contributor rows; there the
//!   diagnostics count is the only signal. Every pack lists `events` first in
//!   `include` so the budget funds events before derived layers.

use serde_json::Value;

/// The warning that marks a storage-order sample of a parent scope's children.
pub(crate) const PARENT_SAMPLE_WARNING: &str = "parent_pack_unranked_sample";

/// One thing a pack says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Note<'a> {
    /// A `warnings[]` entry.
    Warning(&'a str),
    /// The parent-scope sample warning.
    ParentSample(&'a str),
    /// The knapsack evicted items: the diagnostics count, when reported,
    /// and the rendered events named as evicted from `layers.events`.
    Evicted {
        /// `diagnostics.knapsack_evictions`, when present and non-zero.
        count: Option<u64>,
        /// Event ids marked `evicted_from_layers: true`.
        events: Vec<&'a str>,
    },
}

impl Note<'_> {
    /// The level the note is logged at.
    pub(crate) fn level(&self) -> log::Level {
        match self {
            Self::Warning(_) => log::Level::Debug,
            Self::ParentSample(_) | Self::Evicted { .. } => log::Level::Warn,
        }
    }
}

/// Every note `pack` carries, in a fixed order: warnings, then evictions.
pub(crate) fn notes(pack: &Value) -> Vec<Note<'_>> {
    let mut notes: Vec<Note<'_>> = pack
        .get("warnings")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(|warning| {
            if is_parent_sample(warning) {
                Note::ParentSample(warning)
            } else {
                Note::Warning(warning)
            }
        })
        .collect();
    let count = pack
        .pointer("/diagnostics/knapsack_evictions")
        .and_then(Value::as_u64)
        .filter(|count| *count > 0);
    let events: Vec<&str> = pack
        .pointer("/provenance/trail")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("context_contributors")?.as_array())
        .flatten()
        .filter(|row| row.get("evicted_from_layers").and_then(Value::as_bool) == Some(true))
        .filter_map(|row| row.get("event_id")?.as_str())
        .collect();
    if count.is_some() || !events.is_empty() {
        notes.push(Note::Evicted { count, events });
    }
    notes
}

/// Whether `warning` is the parent-scope sample warning: its code alone, or
/// its code followed by `:` and the detail. Another code that merely starts
/// the same way is not.
fn is_parent_sample(warning: &str) -> bool {
    warning
        .strip_prefix(PARENT_SAMPLE_WARNING)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(':'))
}

/// Logs every note `pack` carries for `scope`. The scope and every warning
/// are written with `Debug` escaping: both come from outside this process
/// (the request, the server's answer), and a raw newline in either would
/// forge a log line.
pub(crate) fn report(scope: &str, pack: &Value) {
    for note in notes(pack) {
        let level = note.level();
        match note {
            Note::Warning(warning) => {
                log::log!(
                    level,
                    "[cortex] recall pack warning scope={scope:?} warning={warning:?}"
                );
            }
            Note::ParentSample(warning) => log::log!(
                level,
                "[cortex] recall pack is an unranked parent-scope sample scope={scope:?} \
                 warning={warning:?}"
            ),
            Note::Evicted { count, events } => log::log!(
                level,
                "[cortex] recall pack evicted items to fit its token budget scope={scope:?} \
                 knapsack_evictions={} evicted_events={}",
                count.map_or_else(|| "unreported".to_string(), |count| count.to_string()),
                events.len()
            ),
        }
    }
}

#[cfg(test)]
#[path = "notes_tests.rs"]
mod tests;
