//! Linear host normalization helpers — result extraction and issue-title and
//! timestamp extraction.
//!
//! Linear's GraphQL API (and therefore Composio's wrapping of it) returns
//! connection-style lists (`{ nodes: [...], pageInfo: {...} }`) at the top
//! level or nested under `data`. The functions here walk the union of
//! common shapes so the provider does not have to branch per Composio
//! envelope variant.

use serde_json::Value;

use super::fields::pick_str;

/// Walk the Composio response envelope for Linear issue list results.
///
/// Linear's list endpoints return `{ nodes: [...] }` or
/// `{ issues: { nodes: [...] } }` shapes; Composio may re-wrap the
/// upstream payload under `data` or `data.data`. We probe each shape
/// in order and return the first array we find.
pub fn extract_issues(data: &Value) -> Vec<Value> {
    let candidates = [
        data.pointer("/data/nodes"),
        data.pointer("/nodes"),
        data.pointer("/data/issues/nodes"),
        data.pointer("/issues/nodes"),
        data.pointer("/data/data/nodes"),
        data.pointer("/data/data/issues/nodes"),
        data.pointer("/data/results"),
        data.pointer("/results"),
        data.pointer("/data/items"),
        data.pointer("/items"),
    ];
    for cand in candidates.into_iter().flatten() {
        if let Some(arr) = cand.as_array() {
            return arr.clone();
        }
    }
    Vec::new()
}

/// Extract a human-readable title from a Linear issue object.
///
/// Linear issues store the name at `title` (or `data.title` after
/// Composio envelope wrapping). Falls back to `name` / `identifier`
/// so the chunk remains identifiable even for unusual response shapes.
pub fn extract_issue_title(issue: &Value) -> Option<String> {
    pick_str(
        issue,
        &[
            "title",
            "data.title",
            "name",
            "data.name",
            "identifier",
            "data.identifier",
        ],
    )
}

/// Extract a stable cursor timestamp from a Linear issue object.
///
/// Linear uses ISO-8601 strings for timestamps (`updatedAt`). We keep
/// the value as a string so lexicographic comparison against the stored
/// cursor is valid.
pub fn extract_issue_updated(issue: &Value) -> Option<String> {
    pick_str(
        issue,
        &[
            "updatedAt",
            "data.updatedAt",
            "updated_at",
            "data.updated_at",
        ],
    )
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
