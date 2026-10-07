//! The export check: an engine that exports hands every stored item back
//! whole, exactly as stored, under its own id, and storing it again where it
//! was is a replay. An engine that does not export refuses with
//! `Unsupported`.

use std::collections::HashSet;

use crate::{Error as ApiError, Exported, ListRequest, MetaFilter};

use super::{Ctx, MAX_PAGES, ensure};
use crate::conformance::error::{Error, Result};

/// Runs the export check (see the module docs).
///
/// # Errors
///
/// [`Error::Check`] when the engine exports wrongly, and [`Error::Engine`]
/// when a call it must serve fails.
pub(super) async fn export(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "export";
    let items = ctx.run.round_trip_items();
    for item in &items {
        ctx.call(CHECK, ctx.engine.store(item.clone())).await?;
    }
    let Some(exported) = export_all(ctx, CHECK, &ctx.run.filter()).await? else {
        return Ok(());
    };
    // The run's filter admits exactly these items at this point of the
    // suite (round_trip stored the same three), so anything more is an
    // item the filter should not have admitted, or one handed back twice.
    ensure(CHECK, exported.len() == items.len(), || {
        format!(
            "exported {} items for a filter admitting {}: {:?}",
            exported.len(),
            items.len(),
            exported.iter().map(|e| e.id.as_str()).collect::<Vec<_>>()
        )
    })?;
    for item in &items {
        let id = item.fingerprint();
        let found: Vec<&Exported> = exported.iter().filter(|e| e.id.as_str() == id).collect();
        ensure(CHECK, found.len() == 1, || {
            format!(
                "a stored {:?} was exported {} times under its id",
                item.kind(),
                found.len()
            )
        })?;
        ensure(CHECK, &found[0].item == item, || {
            format!(
                "an exported {:?} is not the item stored: stored {item:?}, exported {:?}",
                item.kind(),
                found[0].item
            )
        })?;
        let again = ctx
            .call(CHECK, ctx.engine.store(found[0].item.clone()))
            .await?;
        ensure(CHECK, again.replayed && again.id.as_str() == id, || {
            format!(
                "storing an exported {:?} where it was is not a replay",
                item.kind()
            )
        })?;
    }
    ensure(
        CHECK,
        exported
            .iter()
            .all(|e| e.id.as_str() == e.item.fingerprint()),
        || "an exported item's id is not its fingerprint".to_string(),
    )
}

/// Every item `filter` admits, exported two at a time so paging is
/// exercised; `None` when the engine does not export.
pub(super) async fn export_all(
    ctx: &Ctx<'_>,
    check: &'static str,
    filter: &MetaFilter,
) -> Result<Option<Vec<Exported>>> {
    let mut all = Vec::new();
    let mut cursor: Option<String> = None;
    let mut seen_cursors = HashSet::new();
    for _ in 0..MAX_PAGES {
        let mut request = ListRequest::new(filter.clone(), 2);
        request.cursor = cursor.clone();
        let page = match ctx.engine.export(request).await {
            Ok(page) => page,
            Err(ApiError::Unsupported(_)) if all.is_empty() && cursor.is_none() => {
                return Ok(None);
            }
            Err(source) => return Err(Error::Engine { check, source }),
        };
        ensure(check, page.incomplete.is_empty(), || {
            format!(
                "whole items were exported as incomplete: {:?}",
                page.incomplete
            )
        })?;
        all.extend(page.items);
        match page.next_cursor {
            Some(next) => {
                ensure(check, seen_cursors.insert(next.clone()), || {
                    format!("the export cursor `{next}` repeated; paging never ends")
                })?;
                cursor = Some(next);
            }
            None => return Ok(Some(all)),
        }
    }
    Err(Error::Check {
        check,
        detail: format!("export did not end within {MAX_PAGES} pages"),
    })
}
