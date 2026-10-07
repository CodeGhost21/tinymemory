//! The read tools: `memory_recall`, `memory_fetch`, `memory_list`,
//! `memory_get` and `memory_explore`.
//!
//! Each parses its arguments strictly ([`crate::tools::args`]), confines the
//! request to the host's reach ([`ToolScope::confine`]), calls the engine,
//! and renders the compact result ([`crate::tools::render`]). The reach in
//! the request is always the scope's, whatever the arguments said: a model
//! cannot name one, and a filter it builds cannot widen one.

use serde_json::Value;
use tinymemory_api::{
    Error, ExploreRequest, FetchRequest, GetRequest, Hit, ItemId, ListRequest, MemoryEngine,
    RecallRequest, Result,
};

use super::ToolScope;
use super::args::{Args, facet, fetch_mode, meta_filter};
use super::render;
use super::spec::schema::{DEFAULT_LIMIT, MAX_IDS, MAX_LIMIT, default_mode};
use super::spec::{MEMORY_EXPLORE, MEMORY_FETCH, MEMORY_GET, MEMORY_LIST, MEMORY_RECALL};

/// `memory_recall`.
///
/// # Errors
///
/// Invalid arguments, and the engine's own failures.
pub(crate) async fn recall(
    engine: &dyn MemoryEngine,
    scope: &ToolScope,
    value: &Value,
) -> Result<Value> {
    let args = Args::parse(
        MEMORY_RECALL,
        value,
        &["question", "filter", "limit", "instructions"],
    )?;
    let mut filter = meta_filter(&args, "filter")?;
    scope.confine(&mut filter);
    let request = RecallRequest {
        question: args.required_string("question")?,
        filter,
        limit: args.count("limit", DEFAULT_LIMIT, MAX_LIMIT)?,
        instructions: args.string("instructions")?,
        refers_to: None,
    };
    Ok(render::recall(&engine.recall(request).await?))
}

/// `memory_fetch`.
///
/// # Errors
///
/// [`Error::Unsupported`] when the engine serves no fetch mode, invalid
/// arguments (a mode the engine does not serve among them), and the engine's
/// own failures.
pub(crate) async fn fetch(
    engine: &dyn MemoryEngine,
    scope: &ToolScope,
    value: &Value,
) -> Result<Value> {
    let modes = &engine.descriptor().fetch_modes;
    let Some(default) = default_mode(modes) else {
        return Err(Error::Unsupported(format!(
            "{MEMORY_FETCH}: engine `{}` serves no fetch mode",
            engine.descriptor().id
        )));
    };
    let args = Args::parse(
        MEMORY_FETCH,
        value,
        &["query", "mode", "filter", "limit", "cursor"],
    )?;
    let mut filter = meta_filter(&args, "filter")?;
    scope.confine(&mut filter);
    let request = FetchRequest {
        query: args.required_string("query")?,
        mode: fetch_mode(&args, "mode", modes, default)?,
        filter,
        limit: args.count("limit", DEFAULT_LIMIT, MAX_LIMIT)?,
        cursor: args.string("cursor")?,
        beliefs: 0,
        refers_to: None,
    };
    Ok(render::fetch(&engine.fetch(request).await?))
}

/// `memory_list`.
///
/// # Errors
///
/// Invalid arguments, and the engine's own failures.
pub(crate) async fn list(
    engine: &dyn MemoryEngine,
    scope: &ToolScope,
    value: &Value,
) -> Result<Value> {
    let args = Args::parse(MEMORY_LIST, value, &["filter", "limit", "cursor"])?;
    let mut filter = meta_filter(&args, "filter")?;
    scope.confine(&mut filter);
    let request = ListRequest {
        filter,
        limit: args.count("limit", DEFAULT_LIMIT, MAX_LIMIT)?,
        cursor: args.string("cursor")?,
    };
    Ok(render::list(&engine.list(request).await?))
}

/// `memory_get`: the items in reach, and the ids that named nothing in reach
/// as `missing`.
///
/// # Errors
///
/// Invalid arguments, and the engine's own failures.
pub(crate) async fn get(
    engine: &dyn MemoryEngine,
    scope: &ToolScope,
    value: &Value,
) -> Result<Value> {
    let args = Args::parse(MEMORY_GET, value, &["ids"])?;
    let ids = item_ids(&args, "ids")?;
    let found = resolve(engine, scope, &ids).await?;
    let missing: Vec<ItemId> = ids
        .into_iter()
        .filter(|id| !found.iter().any(|hit| &hit.id == id))
        .collect();
    Ok(render::get(&found, &missing))
}

/// `memory_explore`.
///
/// # Errors
///
/// Invalid arguments, and the engine's own failures.
pub(crate) async fn explore(
    engine: &dyn MemoryEngine,
    scope: &ToolScope,
    value: &Value,
) -> Result<Value> {
    let args = Args::parse(MEMORY_EXPLORE, value, &["facet", "filter", "limit"])?;
    let mut request = ExploreRequest::new(
        facet(&args, "facet")?,
        args.count("limit", DEFAULT_LIMIT, MAX_LIMIT)?,
    );
    request.filter = meta_filter(&args, "filter")?;
    scope.confine(&mut request.filter);
    Ok(render::explore(&engine.explore(request).await?))
}

/// Reads `ids` with [`MemoryEngine::get`] under the scope's reach: what comes
/// back is exactly what the scope may see.
///
/// # Errors
///
/// The engine's own failures.
pub(crate) async fn resolve(
    engine: &dyn MemoryEngine,
    scope: &ToolScope,
    ids: &[ItemId],
) -> Result<Vec<Hit>> {
    engine
        .get(GetRequest {
            ids: ids.to_vec(),
            reach: scope.reach.clone(),
        })
        .await
}

/// The required id list at `key`: `1..=`[`MAX_IDS`] non-blank strings,
/// duplicates dropped.
///
/// # Errors
///
/// [`Error::InvalidRequest`] for a missing, empty, oversized or blank list.
pub(crate) fn item_ids(args: &Args<'_>, key: &str) -> Result<Vec<ItemId>> {
    let raw = args.strings(key)?;
    if raw.is_empty() || raw.len() > MAX_IDS {
        return Err(args.field_error(key, &format!("must list from 1 to {MAX_IDS} ids")));
    }
    if raw.iter().any(|id| id.trim().is_empty()) {
        return Err(args.field_error(key, "must not hold a blank id"));
    }
    let mut ids: Vec<ItemId> = Vec::with_capacity(raw.len());
    for id in raw {
        let id = ItemId::new(id);
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    Ok(ids)
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
