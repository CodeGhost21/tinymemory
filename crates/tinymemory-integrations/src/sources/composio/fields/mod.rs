//! Field lookup shared by the Composio normalisers: pull a string out of a
//! payload by trying several dotted paths, because Composio wraps the same
//! upstream field at different depths depending on the action and version.

/// Walk a JSON object using a list of dotted-path candidates and return the
/// first non-empty **string** match, trimmed.
///
/// Each path is split on `.` and followed with `Value::get`, so it only
/// descends through objects — it never indexes into an array. A leaf that is
/// not a string (a number, a bool) is rejected rather than coerced, so a
/// payload whose `id` is `42` rather than `"42"` yields `None` here. That
/// differs from the private `scalar` lookup in the `documents` mapping, which
/// renders numbers; the normalisers were written against the
/// reject-non-strings behaviour and `pick_str_rejects_non_string_values` pins
/// it.
pub fn pick_str(value: &serde_json::Value, paths: &[&str]) -> Option<String> {
    for path in paths {
        let mut cur = value;
        let mut ok = true;
        for segment in path.split('.') {
            match cur.get(segment) {
                Some(next) => cur = next,
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        if let Some(s) = cur.as_str() {
            let trimmed = s.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
