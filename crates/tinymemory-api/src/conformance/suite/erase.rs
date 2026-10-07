//! The erase check: an engine that erases removes every item at a node and
//! below it, leaves a sibling alone, and lets an erased item be stored anew
//! (a replay would store nothing). An erasure of the whole tree without its
//! interlock is refused. An engine that does not erase refuses with
//! `Unsupported`.

use crate::{
    EraseRequest, Error as ApiError, ItemKind, LearningKind, MetaFilter, Namespace, Reach,
    StoreItem,
};

use super::{Ctx, ensure};
use crate::conformance::error::{Error, Result};

const CHECK: &str = "erase";

/// The tag isolating this check's items from the rest of the run.
const TAG: &str = "erase";

pub(super) async fn erase(ctx: &Ctx<'_>) -> Result<()> {
    let parse = |value: String| {
        value.parse::<Namespace>().map_err(|source| Error::Engine {
            check: CHECK,
            source,
        })
    };
    let marker = &ctx.run.marker;
    let node = parse(format!("agent:{marker}-erased"))?;
    let child = parse(format!("agent:{marker}-erased/agent:child"))?;
    let sibling = parse(format!("agent:{marker}-kept"))?;
    let item = |label: &str, namespace: &Namespace| {
        let mut meta = ctx.run.meta();
        meta.namespace = namespace.clone();
        meta.tags = vec![TAG.to_string()];
        StoreItem::learning(
            format!("{marker} {label} erasable fact"),
            LearningKind::Fact,
            0.9,
            meta,
        )
    };
    let (at_node, at_child, at_sibling) = (
        item("node", &node),
        item("child", &child),
        item("sibling", &sibling),
    );
    for stored in [&at_node, &at_child, &at_sibling] {
        ctx.call(CHECK, ctx.engine.store(stored.clone())).await?;
    }

    let refused = ctx
        .engine
        .erase(EraseRequest::new(Reach::subtree(Namespace::ROOT)))
        .await;
    ensure(
        CHECK,
        matches!(
            refused,
            Err(ApiError::InvalidRequest(_) | ApiError::Unsupported(_))
        ),
        || format!("erasing the whole tree without its interlock was not refused: {refused:?}"),
    )?;

    match ctx
        .engine
        .erase(EraseRequest::new(Reach::subtree(node.clone())))
        .await
    {
        Ok(_) => {}
        Err(ApiError::Unsupported(_)) => return Ok(()),
        Err(source) => {
            return Err(Error::Engine {
                check: CHECK,
                source,
            });
        }
    }

    let filter = MetaFilter {
        kinds: vec![ItemKind::Learning],
        tags_any: vec![TAG.to_string()],
        ..ctx.run.filter()
    };
    let left: Vec<String> = ctx
        .list_all(CHECK, &filter)
        .await?
        .into_iter()
        .map(|hit| hit.id.as_str().to_string())
        .collect();
    ensure(CHECK, left == vec![at_sibling.fingerprint()], || {
        format!(
            "after erasing a node, expected only its sibling's item, listed {left:?} \
             (node {}, child {}, sibling {})",
            at_node.fingerprint(),
            at_child.fingerprint(),
            at_sibling.fingerprint()
        )
    })?;

    for erased in [&at_node, &at_child] {
        let again = ctx.call(CHECK, ctx.engine.store(erased.clone())).await?;
        ensure(CHECK, !again.replayed, || {
            "storing an erased item again was a replay (nothing stored)".to_string()
        })?;
    }
    let listed = ctx.list_all(CHECK, &filter).await?.len();
    ensure(CHECK, listed == 3, || {
        format!("the erased items stored anew should list again, {listed} of 3 listed")
    })
}
