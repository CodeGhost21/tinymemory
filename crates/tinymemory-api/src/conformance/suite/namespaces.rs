//! The namespace check: every read honours a [`Reach`], so one agent never
//! sees a sibling's memory while both see what the root shares.

use std::collections::BTreeSet;

use crate::{
    ExploreRequest, Facet, FetchRequest, ForgetTarget, GetRequest, ItemId, MetaFilter, Namespace,
    Reach, StoreItem,
};

use super::{Ctx, ensure};
use crate::conformance::error::{Error, Result};

const CHECK: &str = "namespaces";

/// The tag isolating this check's items from the rest of the run.
const TAG: &str = "namespaces";

/// The nodes the check writes to: the root, two sibling agents and one
/// sub-agent below the first.
struct Nodes {
    a: Namespace,
    b: Namespace,
    scout: Namespace,
}

impl Nodes {
    fn for_run(ctx: &Ctx<'_>) -> Result<Self> {
        let parse = |value: String| {
            value.parse::<Namespace>().map_err(|source| Error::Engine {
                check: CHECK,
                source,
            })
        };
        let marker = &ctx.run.marker;
        Ok(Self {
            a: parse(format!("agent:{marker}-a"))?,
            b: parse(format!("agent:{marker}-b"))?,
            scout: parse(format!("agent:{marker}-a/agent:scout"))?,
        })
    }
}

/// Stores one item per node and checks what each reach reads back.
pub(super) async fn namespaces(ctx: &Ctx<'_>) -> Result<()> {
    let nodes = Nodes::for_run(ctx)?;
    let item = |label: &str, namespace: &Namespace| {
        let mut meta = ctx.run.meta();
        meta.namespace = namespace.clone();
        meta.tags = vec![TAG.to_string()];
        StoreItem::learning(
            format!("{} {label} shared fact", ctx.run.marker),
            crate::LearningKind::Fact,
            0.9,
            meta,
        )
    };
    let items = [
        item("root", &Namespace::ROOT),
        item("same", &nodes.a),
        item("same", &nodes.b),
        item("scout", &nodes.scout),
    ];
    let receipts = ctx
        .call(CHECK, ctx.engine.store_many(items.to_vec()))
        .await?;
    let [root, a, b, scout] = [0, 1, 2, 3].map(|i| receipts[i].id.clone());
    ensure(CHECK, a != b, || {
        "the same text in two namespaces stored as one item".to_string()
    })?;

    let cases = [
        (Reach::of(nodes.a.clone()), vec![&root, &a]),
        (Reach::of(nodes.b.clone()), vec![&root, &b]),
        (Reach::of(nodes.scout.clone()), vec![&root, &a, &scout]),
        (Reach::of(Namespace::ROOT), vec![&root]),
        (Reach::exact(nodes.a.clone()), vec![&a]),
        (Reach::subtree(nodes.a.clone()), vec![&a, &scout]),
    ];
    for (reach, wanted) in &cases {
        let got = ids_in(ctx, reach).await?;
        let wanted: BTreeSet<ItemId> = wanted.iter().map(|id| (*id).clone()).collect();
        ensure(CHECK, got == wanted, || {
            format!("reach {reach:?} listed {got:?}, expected {wanted:?}")
        })?;
    }

    let all = vec![root.clone(), a.clone(), b.clone(), scout.clone()];
    let hits = ctx
        .call(
            CHECK,
            ctx.engine.get(GetRequest {
                ids: all.clone(),
                reach: Some(Reach::of(nodes.b.clone())),
            }),
        )
        .await?;
    let got: Vec<&ItemId> = hits.iter().map(|hit| &hit.id).collect();
    ensure(CHECK, got == [&root, &b], || {
        format!("get within agent b's reach returned {got:?}")
    })?;

    if let Some(mode) = ctx.engine.descriptor().fetch_modes.first().copied() {
        let mut req = FetchRequest::new(format!("{} shared fact", ctx.run.marker), mode, 20);
        req.filter = tagged(ctx);
        req.filter.reach = Some(Reach::of(nodes.a.clone()));
        let page = ctx.call(CHECK, ctx.engine.fetch(req)).await?;
        ensure(
            CHECK,
            page.hits.iter().all(|hit| [&root, &a].contains(&&hit.id)),
            || {
                let found: Vec<&ItemId> = page.hits.iter().map(|hit| &hit.id).collect();
                format!("a {mode:?} fetch in agent a's reach found {found:?}")
            },
        )?;
    }

    let mut req = ExploreRequest::new(Facet::Namespace, 10);
    req.filter = tagged(ctx);
    let page = ctx.call(CHECK, ctx.engine.explore(req)).await?;
    let counts: BTreeSet<(String, u64)> = page
        .buckets
        .iter()
        .map(|bucket| (bucket.value.clone(), bucket.count))
        .collect();
    let expected: BTreeSet<(String, u64)> = [&Namespace::ROOT, &nodes.a, &nodes.b, &nodes.scout]
        .into_iter()
        .map(|node| (node.to_string(), 1))
        .collect();
    ensure(CHECK, counts == expected, || {
        format!("the namespace facet counted {counts:?}, expected {expected:?}")
    })?;

    let mut forget = tagged(ctx);
    forget.reach = Some(Reach::exact(nodes.b.clone()));
    let report = ctx
        .call(CHECK, ctx.engine.forget(ForgetTarget::Filter(forget)))
        .await?;
    ensure(CHECK, report.forgotten == 1, || {
        format!(
            "forgetting agent b's node removed {} items",
            report.forgotten
        )
    })?;
    let left = ids_in(ctx, &Reach::subtree(Namespace::ROOT)).await?;
    let wanted: BTreeSet<ItemId> = [root, a, scout].into_iter().collect();
    ensure(CHECK, left == wanted, || {
        format!("after forgetting agent b, {left:?} remain")
    })
}

fn tagged(ctx: &Ctx<'_>) -> MetaFilter {
    MetaFilter {
        tags_any: vec![TAG.to_string()],
        ..ctx.run.filter()
    }
}

async fn ids_in(ctx: &Ctx<'_>, reach: &Reach) -> Result<BTreeSet<ItemId>> {
    let mut filter = tagged(ctx);
    filter.reach = Some(reach.clone());
    Ok(ctx
        .list_all(CHECK, &filter)
        .await?
        .into_iter()
        .map(|hit| hit.id)
        .collect())
}
