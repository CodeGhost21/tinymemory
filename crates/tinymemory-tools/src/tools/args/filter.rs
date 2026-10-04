//! Reading the model-facing filter, the explore facet and the fetch mode.
//!
//! The filter is a subset of [`MetaFilter`]: the fields in
//! [`FILTER_FIELDS`]. Its `reach` is never read from the arguments; the
//! caller sets it from the host's scope afterwards.

use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tinymemory_api::{Error, Facet, FetchMode, ItemKind, MetaFilter, Result, SourceKind};

use super::Args;
use crate::tools::spec::schema::{EXPLORE_FACETS, FILTER_FIELDS};

/// The filter at `key`, or an empty filter when absent. Its `reach` is unset.
///
/// # Errors
///
/// [`Error::InvalidRequest`] naming the offending `filter.*` field.
pub(crate) fn meta_filter(args: &Args<'_>, key: &str) -> Result<MetaFilter> {
    let Some(filter) = args.object(key, "filter.", &FILTER_FIELDS)? else {
        return Ok(MetaFilter::default());
    };
    Ok(MetaFilter {
        kinds: wire_list::<ItemKind>(&filter, "kinds", "an item kind")?,
        sources: wire_list::<SourceKind>(&filter, "sources", "a source kind")?,
        tags_any: filter.strings("tags_any")?,
        workspace: filter.string("workspace")?,
        folder: filter.string("folder")?,
        file_path: filter.string("file_path")?,
        repo: filter.string("repo")?,
        url: filter.string("url")?,
        thread_id: filter.string("thread_id")?,
        agent_id: filter.string("agent_id")?,
        observed_after: timestamp(&filter, "observed_after")?,
        observed_before: timestamp(&filter, "observed_before")?,
        ..MetaFilter::default()
    })
}

/// The required facet at `key`, one of [`EXPLORE_FACETS`].
///
/// # Errors
///
/// [`Error::InvalidRequest`] for a missing value or one outside the list.
pub(crate) fn facet(args: &Args<'_>, key: &str) -> Result<Facet> {
    let name = args.required_string(key)?;
    if !EXPLORE_FACETS.contains(&name.as_str()) {
        return Err(args.field_error(
            key,
            &format!("must be one of {}", EXPLORE_FACETS.join(", ")),
        ));
    }
    wire(&name).ok_or_else(|| args.field_error(key, "is not a facet"))
}

/// The fetch mode at `key`, one of `modes`; `default` when absent.
///
/// # Errors
///
/// [`Error::InvalidRequest`] for a value that is not one of `modes`.
pub(crate) fn fetch_mode(
    args: &Args<'_>,
    key: &str,
    modes: &[FetchMode],
    default: FetchMode,
) -> Result<FetchMode> {
    let Some(name) = args.string(key)? else {
        return Ok(default);
    };
    modes
        .iter()
        .copied()
        .find(|mode| mode.as_str() == name)
        .ok_or_else(|| {
            let names: Vec<&str> = modes.iter().map(|mode| mode.as_str()).collect();
            args.field_error(key, &format!("must be one of {}", names.join(", ")))
        })
}

fn wire_list<T: DeserializeOwned>(args: &Args<'_>, key: &str, what: &str) -> Result<Vec<T>> {
    args.strings(key)?
        .iter()
        .map(|name| {
            wire(name).ok_or_else(|| args.field_error(key, &format!("`{name}` is not {what}")))
        })
        .collect()
}

/// A snake_case wire string read as the enum it names.
fn wire<T: DeserializeOwned>(name: &str) -> Option<T> {
    serde_json::from_value(Value::String(name.to_string())).ok()
}

fn timestamp(args: &Args<'_>, key: &str) -> Result<Option<DateTime<Utc>>> {
    args.string(key)?
        .map(|value| {
            DateTime::parse_from_rfc3339(&value)
                .map(|at| at.with_timezone(&Utc))
                .map_err(|_| args.field_error(key, "must be an rfc 3339 timestamp"))
        })
        .transpose()
}

/// Kept so the error type is named where every function here returns it.
#[allow(dead_code, reason = "documents the error every reader here returns")]
type _Error = Error;

#[cfg(test)]
#[path = "filter_tests.rs"]
mod tests;
