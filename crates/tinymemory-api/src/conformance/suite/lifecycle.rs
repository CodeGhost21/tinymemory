//! The lifecycle checks: the hot-path write and consolidation.
//!
//! - `store_with` — a [`WaitFor::Visible`] store is listed on return, exactly
//!   as `store`; an [`WaitFor::Accepted`] store answers with the item's own
//!   id; and a repeat of a visible item is a replay.
//! - `consolidate` — an invalid request is refused, and a valid one answers
//!   as the descriptor promises: [`Consolidation::None`] refuses with
//!   `Unsupported`, [`Consolidation::OnDemand`] and
//!   [`Consolidation::Automatic`] start or complete a build, and
//!   [`Consolidation::Scheduled`] acknowledges without one. Then
//!   `beliefs` refuses a zero limit, and every belief it returns is a
//!   learning tagged [`BELIEF_TAG`] within the reach asked for.

use crate::{
    BELIEF_TAG, BeliefsRequest, ConsolidateRequest, ConsolidateStatus, Consolidation,
    Error as ApiError, ItemKind, Namespace, Reach, WaitFor, WriteOptions,
};

use super::{Ctx, ensure};
use crate::conformance::error::{Error, Result};

pub(super) async fn store_with(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "store_with";
    let visible = ctx.run.document("store-with-visible", ctx.run.meta());
    let receipt = ctx
        .call(
            CHECK,
            ctx.engine
                .store_with(visible.clone(), WriteOptions::visible()),
        )
        .await?;
    ensure(CHECK, receipt.id.as_str() == visible.fingerprint(), || {
        format!(
            "a visible store answered `{}` for an item fingerprinted `{}`",
            receipt.id.as_str(),
            visible.fingerprint()
        )
    })?;
    let listed = ctx.list_all(CHECK, &ctx.run.filter()).await?;
    ensure(CHECK, listed.iter().any(|hit| hit.id == receipt.id), || {
        "a store waiting for visibility was not listed on return".to_string()
    })?;
    let again = ctx
        .call(
            CHECK,
            ctx.engine.store_with(visible, WriteOptions::visible()),
        )
        .await?;
    ensure(CHECK, again.replayed, || {
        "storing a visible item again was not a replay".to_string()
    })?;

    let accepted = ctx.run.document("store-with-accepted", ctx.run.meta());
    let receipt = ctx
        .call(
            CHECK,
            ctx.engine.store_with(
                accepted.clone(),
                WriteOptions {
                    wait: WaitFor::Accepted,
                },
            ),
        )
        .await?;
    ensure(CHECK, receipt.id.as_str() == accepted.fingerprint(), || {
        format!(
            "an accepted store answered `{}` for an item fingerprinted `{}`",
            receipt.id.as_str(),
            accepted.fingerprint()
        )
    })
}

pub(super) async fn consolidate(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "consolidate";
    let node: Namespace = format!("agent:{}-beliefs", ctx.run.marker)
        .parse()
        .map_err(|source| Error::Engine {
            check: CHECK,
            source,
        })?;
    let mut meta = ctx.run.meta();
    meta.namespace = node.clone();
    ctx.call(
        CHECK,
        ctx.engine
            .store(ctx.run.document("consolidate. A fact to build on.", meta)),
    )
    .await?;

    let invalid = ConsolidateRequest::new(Reach::exact(node.clone()))
        .kinds([ItemKind::Document, ItemKind::Document]);
    let refused = ctx.engine.consolidate(invalid).await;
    ensure(
        CHECK,
        matches!(refused, Err(ApiError::InvalidRequest(_))),
        || format!("a request naming a kind twice was not refused: {refused:?}"),
    )?;

    let request = ConsolidateRequest::new(Reach::exact(node.clone())).kinds([ItemKind::Document]);
    let outcome = ctx.engine.consolidate(request).await;
    let promised = ctx.engine.descriptor().consolidation;
    answers_as_promised(promised, outcome)?;
    beliefs(ctx, node).await
}

/// A consolidation's answer against what the descriptor promised.
fn answers_as_promised(
    promised: Consolidation,
    outcome: std::result::Result<crate::ConsolidateReceipt, ApiError>,
) -> Result<()> {
    const CHECK: &str = "consolidate";
    match (promised, outcome) {
        (Consolidation::None, Err(ApiError::Unsupported(_))) => Ok(()),
        (Consolidation::None, other) => Err(Error::Check {
            check: CHECK,
            detail: format!("an engine declaring no consolidation answered {other:?}"),
        }),
        (_, Err(source)) => Err(Error::Engine {
            check: CHECK,
            source,
        }),
        (Consolidation::OnDemand | Consolidation::Automatic, Ok(receipt)) => ensure(
            CHECK,
            matches!(
                receipt.status,
                ConsolidateStatus::Started | ConsolidateStatus::Completed
            ),
            || format!("a {promised:?} engine answered {:?}", receipt.status),
        ),
        (Consolidation::Scheduled, Ok(receipt)) => ensure(
            CHECK,
            receipt.status == ConsolidateStatus::Scheduled,
            || format!("a scheduled engine answered {:?}", receipt.status),
        ),
    }
}

/// Beliefs read back after a build: a malformed request is refused, and
/// every belief is a tagged learning within the reach asked for.
async fn beliefs(ctx: &Ctx<'_>, node: Namespace) -> Result<()> {
    const CHECK: &str = "consolidate";
    let reach = Reach::exact(node);
    let refused = ctx
        .engine
        .beliefs(BeliefsRequest::new(reach.clone(), 0))
        .await;
    ensure(
        CHECK,
        matches!(refused, Err(ApiError::InvalidRequest(_))),
        || format!("a beliefs read with a zero limit was not refused: {refused:?}"),
    )?;
    let held = ctx
        .call(
            CHECK,
            ctx.engine
                .beliefs(BeliefsRequest::new(reach.clone(), 10).query("A fact to build on")),
        )
        .await?;
    ensure(CHECK, held.len() <= 10, || {
        format!("a beliefs read for 10 answered {}", held.len())
    })?;
    for belief in held {
        ensure(
            CHECK,
            belief.kind == ItemKind::Learning
                && belief.meta.tags.iter().any(|tag| tag == BELIEF_TAG)
                && reach.admits(&belief.meta.namespace),
            || {
                format!(
                    "a belief must be a learning tagged `{BELIEF_TAG}` within its reach: {belief:?}"
                )
            },
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[path = "lifecycle_tests.rs"]
mod tests;
