//! The individual checks [`super::run`] performs, in order.

use std::collections::BTreeSet;

use crate::{
    Error as ApiError, FetchMode, FetchRequest, ForgetTarget, ItemId, MemoryMeta, MetaFilter,
    RecallRequest,
};

use super::{Ctx, ensure};
use crate::conformance::error::Result;

/// Every check, stopping at the first failure.
pub(super) async fn all(ctx: &Ctx<'_>) -> Result<()> {
    health(ctx).await?;
    round_trip(ctx).await?;
    super::export::export(ctx).await?;
    replay(ctx).await?;
    super::explore::explore(ctx).await?;
    super::explore::get(ctx).await?;
    super::bulk::store_many(ctx).await?;
    super::lifecycle::store_with(ctx).await?;
    fetch_filters(ctx).await?;
    unsupported_modes(ctx).await?;
    super::namespaces::namespaces(ctx).await?;
    empty_forget(ctx).await?;
    forget_by_id(ctx).await?;
    forget_by_filter(ctx).await?;
    recall(ctx).await?;
    super::lifecycle::consolidate(ctx).await
}

/// Forgets everything the run stored and checks it is gone.
pub(super) async fn cleanup(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "cleanup";
    for filter in [ctx.run.filter(), ctx.run.probe_filter()] {
        ctx.call(
            CHECK,
            ctx.engine.forget(ForgetTarget::Filter(filter.clone())),
        )
        .await?;
        let left = ctx.list_all(CHECK, &filter).await?;
        ensure(CHECK, left.is_empty(), || {
            format!("{} items survived a forget by workspace filter", left.len())
        })?;
    }
    Ok(())
}

async fn health(ctx: &Ctx<'_>) -> Result<()> {
    let health = ctx.engine.health().await;
    ensure("health", health.is_serving(), || {
        format!("the engine reports {health:?}")
    })
}

async fn round_trip(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "round_trip";
    for item in ctx.run.round_trip_items() {
        let receipt = ctx.call(CHECK, ctx.engine.store(item.clone())).await?;
        ensure(CHECK, !receipt.replayed, || {
            format!("a first store of a {:?} reported a replay", item.kind())
        })?;
        let filter = MetaFilter {
            kinds: vec![item.kind()],
            ..ctx.run.filter()
        };
        let listed = ctx.list_all(CHECK, &filter).await?;
        let found: Vec<_> = listed.iter().filter(|hit| hit.id == receipt.id).collect();
        ensure(CHECK, found.len() == 1, || {
            format!(
                "a stored {:?} listed {} times under its receipt id",
                item.kind(),
                found.len()
            )
        })?;
        let hit = found[0];
        ensure(CHECK, hit.kind == item.kind(), || {
            format!("stored a {:?}, listed a {:?}", item.kind(), hit.kind)
        })?;
        ensure(CHECK, &hit.meta == item.meta(), || {
            format!(
                "metadata changed: stored {:?}, listed {:?}",
                item.meta(),
                hit.meta
            )
        })?;
        ensure(CHECK, hit.confidence == item.confidence(), || {
            format!(
                "confidence changed: stored {:?}, listed {:?}",
                item.confidence(),
                hit.confidence
            )
        })?;
        ensure(CHECK, hit.text == item.render_text(), || {
            format!(
                "text changed: stored {:?}, listed {:?}",
                item.render_text(),
                hit.text
            )
        })?;
        ensure(
            CHECK,
            listed.iter().all(|hit| hit.kind == item.kind()),
            || "a kind-filtered listing returned another kind".to_string(),
        )?;
    }
    Ok(())
}

async fn replay(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "replay";
    let item = ctx.run.document("replay", ctx.run.meta());
    let first = ctx.call(CHECK, ctx.engine.store(item.clone())).await?;
    let second = ctx.call(CHECK, ctx.engine.store(item)).await?;
    ensure(CHECK, second.replayed, || {
        "an identical retry was not reported as a replay".to_string()
    })?;
    ensure(CHECK, first.id == second.id, || {
        format!("a replay changed the id from {} to {}", first.id, second.id)
    })?;
    let copies = ctx
        .list_all(CHECK, &ctx.run.filter())
        .await?
        .into_iter()
        .filter(|hit| hit.id == first.id)
        .count();
    ensure(CHECK, copies == 1, || {
        format!("an identical retry left {copies} copies")
    })
}

async fn fetch_filters(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "fetch_filters";
    let probes = ctx.run.probes();
    let mut ids = Vec::with_capacity(probes.len());
    for probe in &probes {
        ids.push(
            ctx.call(CHECK, ctx.engine.store(probe.item.clone()))
                .await?
                .id,
        );
    }
    let modes = ctx.engine.descriptor().fetch_modes.clone();
    for (probe, id) in probes.iter().zip(&ids) {
        for mode in &modes {
            let mut request = FetchRequest::new(ctx.run.marker.clone(), *mode, 50);
            request.filter = probe.filter.clone();
            let page = ctx.call(CHECK, ctx.engine.fetch(request)).await?;
            let found: BTreeSet<&ItemId> = page.hits.iter().map(|hit| &hit.id).collect();
            ensure(CHECK, found == BTreeSet::from([id]), || {
                format!(
                    "{} fetch filtered by `{}` returned {found:?}, expected only {id}",
                    mode.as_str(),
                    probe.field
                )
            })?;
        }
        let listed = ctx.list_all(CHECK, &probe.filter).await?;
        let found: BTreeSet<&ItemId> = listed.iter().map(|hit| &hit.id).collect();
        ensure(CHECK, found == BTreeSet::from([id]), || {
            format!(
                "list filtered by `{}` returned {found:?}, expected only {id}",
                probe.field
            )
        })?;
    }
    for mode in &modes {
        let mut request = FetchRequest::new(ctx.run.marker.clone(), *mode, 50);
        request.filter = ctx.run.probe_filter();
        let page = ctx.call(CHECK, ctx.engine.fetch(request)).await?;
        let found: BTreeSet<&ItemId> = page.hits.iter().map(|hit| &hit.id).collect();
        ensure(CHECK, ids.iter().all(|id| found.contains(id)), || {
            format!(
                "{} fetch filtered by workspace missed probes: found {found:?}",
                mode.as_str()
            )
        })?;
    }
    Ok(())
}

async fn unsupported_modes(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "unsupported_modes";
    for mode in FetchMode::ALL {
        if ctx.engine.descriptor().supports(mode) {
            continue;
        }
        let result = ctx
            .engine
            .fetch(FetchRequest::new(ctx.run.marker.clone(), mode, 5))
            .await;
        ensure(
            CHECK,
            matches!(result, Err(ApiError::Unsupported(_))),
            || format!("an undeclared {} fetch answered {result:?}", mode.as_str()),
        )?;
    }
    Ok(())
}

async fn empty_forget(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "empty_forget";
    let before = ctx.list_all(CHECK, &ctx.run.filter()).await?.len();
    for target in [
        ForgetTarget::Ids(Vec::new()),
        ForgetTarget::Filter(MetaFilter::default()),
    ] {
        let result = ctx.engine.forget(target.clone()).await;
        ensure(
            CHECK,
            matches!(result, Err(ApiError::InvalidRequest(_))),
            || format!("forget({target:?}) answered {result:?}, not an invalid request"),
        )?;
    }
    let after = ctx.list_all(CHECK, &ctx.run.filter()).await?.len();
    ensure(CHECK, before == after, || {
        format!("a refused forget changed the item count from {before} to {after}")
    })
}

async fn forget_by_id(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "forget_by_id";
    let keep = ctx.list_all(CHECK, &ctx.run.filter()).await?.len();
    let item = ctx.run.document("forget by id", ctx.run.meta());
    let id = ctx.call(CHECK, ctx.engine.store(item)).await?.id;
    let report = ctx
        .call(
            CHECK,
            ctx.engine.forget(ForgetTarget::Ids(vec![id.clone()])),
        )
        .await?;
    ensure(CHECK, report.forgotten == 1, || {
        format!("forgetting one id reported {} forgotten", report.forgotten)
    })?;
    let listed = ctx.list_all(CHECK, &ctx.run.filter()).await?;
    ensure(CHECK, listed.iter().all(|hit| hit.id != id), || {
        format!("item {id} still lists after it was forgotten")
    })?;
    ensure(CHECK, listed.len() == keep, || {
        format!(
            "forgetting one id changed the other items: {keep} became {}",
            listed.len()
        )
    })?;
    let again = ctx
        .call(CHECK, ctx.engine.forget(ForgetTarget::Ids(vec![id])))
        .await?;
    ensure(CHECK, again.forgotten == 0, || {
        format!(
            "forgetting a gone id reported {} forgotten",
            again.forgotten
        )
    })
}

async fn forget_by_filter(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "forget_by_filter";
    let keep = ctx.list_all(CHECK, &ctx.run.filter()).await?.len();
    let tag = format!("{}-drop", ctx.run.marker);
    for label in ["drop one", "drop two"] {
        let meta = MemoryMeta {
            tags: vec![tag.clone()],
            ..ctx.run.meta()
        };
        ctx.call(CHECK, ctx.engine.store(ctx.run.document(label, meta)))
            .await?;
    }
    let filter = MetaFilter {
        tags_any: vec![tag],
        ..ctx.run.filter()
    };
    let report = ctx
        .call(
            CHECK,
            ctx.engine.forget(ForgetTarget::Filter(filter.clone())),
        )
        .await?;
    ensure(CHECK, report.forgotten == 2, || {
        format!("forgetting two tagged items reported {}", report.forgotten)
    })?;
    let left = ctx.list_all(CHECK, &filter).await?;
    ensure(CHECK, left.is_empty(), || {
        format!("{} tagged items survived their forget", left.len())
    })?;
    let after = ctx.list_all(CHECK, &ctx.run.filter()).await?.len();
    ensure(CHECK, after == keep, || {
        format!("a filtered forget touched other items: {keep} became {after}")
    })
}

async fn recall(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "recall";
    let mut request = RecallRequest::new(ctx.run.marker.clone(), 5);
    request.filter = ctx.run.filter();
    let answer = ctx.call(CHECK, ctx.engine.recall(request)).await?;
    ensure(CHECK, !answer.answer.trim().is_empty(), || {
        "the answer is empty".to_string()
    })?;
    ensure(CHECK, !answer.citations.is_empty(), || {
        "an answer over matching items cited nothing".to_string()
    })?;
    ensure(CHECK, answer.citations.len() <= 5, || {
        format!("{} citations exceed the limit of 5", answer.citations.len())
    })?;
    let listed = ctx.list_all(CHECK, &ctx.run.filter()).await?;
    for citation in &answer.citations {
        let resolved = listed.iter().find(|hit| hit.id == citation.id);
        ensure(
            CHECK,
            resolved.is_some_and(|hit| hit.kind == citation.kind),
            || {
                format!(
                    "citation {} ({:?}) does not resolve through list",
                    citation.id, citation.kind
                )
            },
        )?;
    }
    Ok(())
}
