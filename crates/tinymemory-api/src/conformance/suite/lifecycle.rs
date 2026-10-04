//! The lifecycle checks: the hot-path write and consolidation.
//!
//! - `store_with` — a [`WaitFor::Visible`] store is listed on return, exactly
//!   as `store`; an [`WaitFor::Accepted`] store answers with the item's own
//!   id; and a repeat of a visible item is a replay.
//! - `consolidate` — an invalid request is refused, and a valid one answers
//!   as the descriptor promises: [`Consolidation::None`] refuses with
//!   `Unsupported`, [`Consolidation::OnDemand`] starts or completes a build,
//!   and [`Consolidation::Scheduled`] acknowledges without one.

use crate::{
    ConsolidateRequest, ConsolidateStatus, Consolidation, Error as ApiError, ItemKind, Namespace,
    Reach, WaitFor, WriteOptions,
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

    let request = ConsolidateRequest::new(Reach::exact(node)).kinds([ItemKind::Document]);
    let outcome = ctx.engine.consolidate(request).await;
    let promised = ctx.engine.descriptor().consolidation;
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
        (Consolidation::OnDemand, Ok(receipt)) => ensure(
            CHECK,
            matches!(
                receipt.status,
                ConsolidateStatus::Started | ConsolidateStatus::Completed
            ),
            || format!("an on-demand engine answered {:?}", receipt.status),
        ),
        (Consolidation::Scheduled, Ok(receipt)) => ensure(
            CHECK,
            receipt.status == ConsolidateStatus::Scheduled,
            || format!("a scheduled engine answered {:?}", receipt.status),
        ),
    }
}
