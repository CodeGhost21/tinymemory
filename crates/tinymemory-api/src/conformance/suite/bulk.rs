//! The bulk check: `store_many` stores in order, every item is listed on
//! return, a repeat is all replays, and an empty batch is refused.

use crate::Error as ApiError;

use super::{Ctx, ensure};
use crate::conformance::error::Result;

pub(super) async fn store_many(ctx: &Ctx<'_>) -> Result<()> {
    const CHECK: &str = "store_many";
    let items: Vec<_> = (0..3)
        .map(|i| ctx.run.document(&format!("bulk-{i}"), ctx.run.meta()))
        .collect();
    let receipts = ctx
        .call(CHECK, ctx.engine.store_many(items.clone()))
        .await?;
    ensure(CHECK, receipts.len() == items.len(), || {
        format!("{} items stored, {} receipts", items.len(), receipts.len())
    })?;
    ensure(CHECK, receipts.iter().all(|r| !r.replayed), || {
        "a first bulk store reported a replay".to_string()
    })?;
    for (item, receipt) in items.iter().zip(&receipts) {
        ensure(CHECK, receipt.id.as_str() == item.fingerprint(), || {
            format!(
                "receipts are out of order: `{}` for an item fingerprinted `{}`",
                receipt.id.as_str(),
                item.fingerprint()
            )
        })?;
    }
    let listed = ctx.list_all(CHECK, &ctx.run.filter()).await?;
    for receipt in &receipts {
        ensure(CHECK, listed.iter().any(|hit| hit.id == receipt.id), || {
            format!(
                "`{}` was not listed when store_many returned",
                receipt.id.as_str()
            )
        })?;
    }
    let again = ctx.call(CHECK, ctx.engine.store_many(items)).await?;
    ensure(CHECK, again.iter().all(|r| r.replayed), || {
        "storing the same batch again was not all replays".to_string()
    })?;
    let refused = ctx.engine.store_many(Vec::new()).await;
    ensure(
        CHECK,
        matches!(refused, Err(ApiError::InvalidRequest(_))),
        || format!("an empty batch was not refused: {refused:?}"),
    )
}
