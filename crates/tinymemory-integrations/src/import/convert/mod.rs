//! Conversions from legacy column values to v2 contract values.
//!
//! Legacy columns are loosely typed: timestamps are float seconds or integer
//! milliseconds, tags and tool calls are JSON text that older writers did not
//! always produce, and roles are free text. Every conversion here is total and
//! lenient: a value that cannot be read becomes "absent" rather than failing
//! the whole import, because one odd row must not strand everything after it.

use serde_json::Value;
use tinymemory_api::chrono::{DateTime, Utc};
use tinymemory_api::{LearningKind, Role, ToolCallRef};

/// Confidence used when a legacy row records none, or an unusable one.
pub(crate) const DEFAULT_CONFIDENCE: f32 = 0.5;

/// A UTC instant from unix seconds with a fractional part (`created_at`,
/// `updated_at`, `timestamp`, `last_seen_at`), rounded to microseconds.
pub(crate) fn from_unix_seconds(seconds: f64) -> Option<DateTime<Utc>> {
    if !seconds.is_finite() {
        return None;
    }
    let micros = (seconds * 1_000_000.0).round();
    if micros.abs() > i64::MAX as f64 {
        return None;
    }
    DateTime::from_timestamp_micros(micros as i64)
}

/// A UTC instant from unix milliseconds (`mem_tree_chunks.timestamp_ms`).
pub(crate) fn from_unix_millis(millis: i64) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp_millis(millis)
}

/// A confidence in `0.0..=1.0`; non-finite or absent values become
/// [`DEFAULT_CONFIDENCE`].
pub(crate) fn confidence(value: Option<f64>) -> f32 {
    match value {
        Some(value) if value.is_finite() => value.clamp(0.0, 1.0) as f32,
        _ => DEFAULT_CONFIDENCE,
    }
}

/// The string elements of a JSON array (`tags_json`); anything else is empty.
pub(crate) fn string_array(json: &str) -> Vec<String> {
    match serde_json::from_str::<Value>(json) {
        Ok(Value::Array(values)) => values
            .into_iter()
            .filter_map(|value| match value {
                Value::String(text) if !text.trim().is_empty() => Some(text),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// A JSON object (`metadata_json`), or `None` when the text is not one.
pub(crate) fn object(json: &str) -> Option<serde_json::Map<String, Value>> {
    match serde_json::from_str::<Value>(json) {
        Ok(Value::Object(map)) => Some(map),
        _ => None,
    }
}

/// A non-blank string field of a JSON object.
pub(crate) fn string_field(map: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    match map.get(key) {
        Some(Value::String(text)) if !text.trim().is_empty() => Some(text.clone()),
        _ => None,
    }
}

/// Maps a free-text `episodic_log.role`.
///
/// Recognised spellings map to their role, case-insensitively. Anything else
/// becomes [`Role::User`]: v1 wrote only `user` and `assistant` itself, so an
/// unknown role came from a host channel, whose speaker is a person rather
/// than the assistant or a tool.
pub(crate) fn role(raw: &str) -> Role {
    match raw.trim().to_ascii_lowercase().as_str() {
        "assistant" | "ai" | "agent" | "bot" | "model" => Role::Assistant,
        "system" | "developer" => Role::System,
        "tool" | "function" | "tool_result" => Role::Tool,
        _ => Role::User,
    }
}

/// Tool calls from `episodic_log.tool_calls_json`.
///
/// Accepts an array of calls or a single call, where a call is an object
/// naming its tool as `name`, `tool`, `tool_name` or `function.name`, with an
/// optional `id` or `call_id`; an object wrapping a `tool_calls` array is
/// unwrapped. Unparseable text and calls without a name are dropped.
pub(crate) fn tool_calls(json: &str) -> Vec<ToolCallRef> {
    match serde_json::from_str::<Value>(json) {
        Ok(value) => calls_in(&value),
        Err(_) => Vec::new(),
    }
}

fn calls_in(value: &Value) -> Vec<ToolCallRef> {
    match value {
        Value::Array(values) => values.iter().filter_map(call).collect(),
        Value::Object(map) => match map.get("tool_calls") {
            Some(Value::Array(values)) => values.iter().filter_map(call).collect(),
            _ => call(value).into_iter().collect(),
        },
        _ => Vec::new(),
    }
}

fn call(value: &Value) -> Option<ToolCallRef> {
    let map = value.as_object()?;
    let name = ["name", "tool", "tool_name"]
        .iter()
        .find_map(|key| string_field(map, key))
        .or_else(|| {
            map.get("function")
                .and_then(Value::as_object)
                .and_then(|function| string_field(function, "name"))
        })?;
    let id = string_field(map, "id").or_else(|| string_field(map, "call_id"));
    Some(ToolCallRef { name, id })
}

/// The learning kind for a v1 learning class.
///
/// `style` and `channel` describe how the user wants things done
/// (preferences); `identity` is a fact about the user; `tooling` is how to
/// do something (procedure); `veto` records something the user rejected
/// (correction); `goal` and unknown classes are [`LearningKind::Other`].
pub(crate) fn learning_kind(class: &str) -> LearningKind {
    match class.trim().to_ascii_lowercase().as_str() {
        "style" | "channel" => LearningKind::Preference,
        "identity" => LearningKind::Fact,
        "tooling" => LearningKind::Procedure,
        "veto" => LearningKind::Correction,
        _ => LearningKind::Other,
    }
}

/// A JSON value as statement text: a string as itself, anything else as
/// compact JSON.
pub(crate) fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
