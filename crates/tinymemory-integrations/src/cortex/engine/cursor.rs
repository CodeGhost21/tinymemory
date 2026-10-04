//! Opaque page cursors.
//!
//! A cursor is a small JSON state, hex-encoded behind a one-letter tag (`l`
//! for a listing, `f` for a fetch), so a host can store and pass it back but
//! not usefully edit it, and a cursor from one operation is refused by the
//! other.

use serde::{Deserialize, Serialize};

use crate::cortex::error::{Error, Result};

/// Where a listing stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct ListCursor {
    /// The path of the scope being listed; `None` before the first. A path
    /// rather than a position, so a scope created between two pages cannot
    /// shift the listing.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) scope: Option<String>,
    /// The engine cursor of the page being read; `None` for the first page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) engine: Option<String>,
    /// How many of that page's raw events were already consumed.
    pub(super) offset: usize,
    /// The id of the last raw event consumed. The engine emits each event
    /// twice in a row and a page boundary can fall between the two copies,
    /// so the next page skips a leading copy of this id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) last: Option<String>,
}

impl ListCursor {
    /// The cursor at the start of the scope at `path`.
    pub(super) fn at(path: &str) -> Self {
        Self {
            scope: Some(path.to_string()),
            ..Self::default()
        }
    }
}

/// Where a fetch stopped: how many ranked hits were already returned.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct FetchCursor {
    pub(super) offset: usize,
}

/// Encodes `state` behind `tag`.
pub(super) fn encode<T: Serialize>(tag: char, state: &T) -> Result<String> {
    let json = serde_json::to_vec(state)
        .map_err(|_| Error::Engine("a page cursor could not be encoded".to_string()))?;
    let mut out = String::with_capacity(json.len() * 2 + 1);
    out.push(tag);
    for byte in json {
        out.push_str(&format!("{byte:02x}"));
    }
    Ok(out)
}

/// Decodes a cursor written by [`encode`] with the same `tag`.
///
/// # Errors
///
/// [`Error::InvalidRequest`] for anything else.
pub(super) fn decode<T: for<'de> Deserialize<'de>>(tag: char, cursor: &str) -> Result<T> {
    let malformed = || Error::InvalidRequest("the page cursor is malformed".to_string());
    let hex = cursor.strip_prefix(tag).ok_or_else(malformed)?;
    if hex.len() % 2 != 0 {
        return Err(malformed());
    }
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| {
            hex.get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
        })
        .collect::<Option<Vec<u8>>>()
        .ok_or_else(malformed)?;
    serde_json::from_slice(&bytes).map_err(|_| malformed())
}

#[cfg(test)]
#[path = "cursor_tests.rs"]
mod tests;
