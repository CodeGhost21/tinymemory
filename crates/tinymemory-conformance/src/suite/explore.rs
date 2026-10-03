//! The explorer checks: `explore` agrees with `list`, and `get` returns what
//! `list` does, by id.

use std::collections::BTreeMap;

use tinymemory_api::{Error as ApiError, ExploreRequest, Facet, GetRequest, ItemId};

use super::{Ctx, ensure};
use crate::error::Result;

/// Groups the run's items by kind and by workspace and checks every count
/// against a listing, then narrows by each bucket and lists again.
pub(super) async fn explore(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "explore";
    let filter = ctx.run.filter();
    let listed = ctx.list_all(CHECK, &filter).await?;
    let mut by_kind: BTreeMap<String, u64> = BTreeMap::new();
    for hit in &listed {
        *by_kind.entry(hit.kind.as_str().to_string()).or_default() += 1;
    }

    let mut req = ExploreRequest::new(Facet::Kind, 10);
    req.filter = filter.clone();
    let page = ctx.call(CHECK, ctx.engine.explore(req)).await?;
    ensure(CHECK, page.facet == Facet::Kind, || {
        format!("asked for the kind facet, got {:?}", page.facet)
    })?;
    ensure(CHECK, !page.truncated, || {
        "a handful of items truncated the scan".to_string()
    })?;
    ensure(CHECK, page.total == listed.len() as u64, || {
        format!("explore saw {} items, list {}", page.total, listed.len())
    })?;
    let explored: BTreeMap<String, u64> = page
        .buckets
        .iter()
        .map(|bucket| (bucket.value.clone(), bucket.count))
        .collect();
    ensure(CHECK, explored == by_kind, || {
        format!("explore counted {explored:?} per kind, list {by_kind:?}")
    })?;
    ensure(
        CHECK,
        page.buckets
            .windows(2)
            .all(|pair| pair[0].count >= pair[1].count),
        || "buckets are not largest first".to_string(),
    )?;

    for bucket in &page.buckets {
        let mut narrowed = filter.clone();
        Facet::Kind
            .narrow(&mut narrowed, &bucket.value)
            .map_err(|source| crate::Error::Engine {
                check: CHECK,
                source,
            })?;
        let items = ctx.list_all(CHECK, &narrowed).await?;
        ensure(CHECK, items.len() as u64 == bucket.count, || {
            format!(
                "the `{}` bucket counts {} but narrows to {} items",
                bucket.value,
                bucket.count,
                items.len()
            )
        })?;
    }

    let mut req = ExploreRequest::new(Facet::Workspace, 10);
    req.filter = filter;
    let page = ctx.call(CHECK, ctx.engine.explore(req)).await?;
    ensure(
        CHECK,
        page.buckets.len() == 1
            && page.buckets[0].value == ctx.run.workspace
            && page.buckets[0].count == listed.len() as u64
            && page.missing == 0,
        || format!("the run's workspace facet was {page:?}"),
    )?;

    let refused = ctx
        .engine
        .explore(ExploreRequest::new(Facet::Kind, 0))
        .await;
    ensure(
        CHECK,
        matches!(refused, Err(ApiError::InvalidRequest(_))),
        || format!("a zero bucket limit was not refused: {refused:?}"),
    )
}

/// Reads the run's items back by id, in a shuffled order with an unknown id
/// among them.
pub(super) async fn get(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "get";
    let listed = ctx.list_all(CHECK, &ctx.run.filter()).await?;
    ensure(CHECK, !listed.is_empty(), || {
        "the run has nothing to read back".to_string()
    })?;
    let mut ids: Vec<ItemId> = listed.iter().rev().map(|hit| hit.id.clone()).collect();
    ids.insert(
        1.min(ids.len()),
        ItemId::new(format!("{}-missing", ctx.run.marker)),
    );
    let hits = ctx
        .call(
            CHECK,
            ctx.engine.get(GetRequest {
                ids: ids.clone(),
                reach: None,
            }),
        )
        .await?;
    let wanted: Vec<&ItemId> = ids
        .iter()
        .filter(|id| listed.iter().any(|hit| &hit.id == *id))
        .collect();
    let got: Vec<&ItemId> = hits.iter().map(|hit| &hit.id).collect();
    ensure(CHECK, got == wanted, || {
        format!("asked for {wanted:?} (and one unknown id), got {got:?}")
    })?;
    for hit in &hits {
        let same = listed.iter().any(|listing| {
            listing.id == hit.id
                && hit.kind == listing.kind
                && hit.meta == listing.meta
                && hit.text == listing.text
        });
        ensure(CHECK, same, || {
            format!(
                "`{}` reads back differently from its listing",
                hit.id.as_str()
            )
        })?;
    }
    let refused = ctx
        .engine
        .get(GetRequest {
            ids: Vec::new(),
            reach: None,
        })
        .await;
    ensure(
        CHECK,
        matches!(refused, Err(ApiError::InvalidRequest(_))),
        || format!("an empty get was not refused: {refused:?}"),
    )?;
    Ok(())
}
