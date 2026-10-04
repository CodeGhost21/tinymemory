//! ClickUp host normalization helpers — result extraction and task-title and
//! timestamp extraction.
//!
//! ClickUp's REST API (and therefore Composio's wrapping of it) returns
//! task lists in a small handful of shapes depending on which endpoint
//! is called. The functions here walk the union of common shapes so the
//! provider doesn't have to branch per Composio envelope variant.

use serde_json::Value;

use super::fields::pick_str;

/// Walk the Composio response envelope for ClickUp task list results.
///
/// ClickUp's "filtered team tasks" endpoint returns `{ "tasks": [...] }`
/// at the top level; Composio re-wraps the upstream payload under
/// `data` or `data.data` depending on the action. We probe each shape
/// in order and return the first array we find.
pub fn extract_tasks(data: &Value) -> Vec<Value> {
    let candidates = [
        data.pointer("/data/tasks"),
        data.pointer("/tasks"),
        data.pointer("/data/data/tasks"),
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

/// Extract a human-readable title from a ClickUp task object.
///
/// ClickUp tasks store the name at `name` (or `data.name` after Composio
/// envelope wrapping). When the name is missing we fall back to the
/// task ID so chunks remain identifiable.
pub fn extract_task_name(task: &Value) -> Option<String> {
    pick_str(task, &["name", "data.name", "title", "data.title"])
}

/// Extract a stable cursor timestamp (milliseconds since epoch as a
/// string) from a ClickUp task object.
///
/// The ClickUp API returns `date_updated` as a stringified epoch ms
/// (e.g. `"1733412345678"`); we keep it as a string so lexicographic
/// comparison against the stored cursor remains valid as long as the
/// length doesn't change (it won't until year 33658).
pub fn extract_task_updated(task: &Value) -> Option<String> {
    pick_str(
        task,
        &[
            "date_updated",
            "data.date_updated",
            "updated_at",
            "data.updated_at",
            "dateUpdated",
            "data.dateUpdated",
        ],
    )
}

#[cfg(test)]
#[path = "clickup_tests.rs"]
mod tests;
