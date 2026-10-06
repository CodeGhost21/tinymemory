//! The agent memory lifecycle (`tinymemory_tools`) against a real CortexDB
//! server.
//!
//! Skipped unless `TINYMEMORY_LIVE_CORTEXDB_URL` names one (see
//! `integration/cortexdb/`); the key defaults to the harness's
//! (`TINYMEMORY_TEST_CORTEX_KEY`). Everything is written below a node unique
//! to the run and forgotten at the end.
//!
//! It proves the hot path on the real wire: a brain document converted and
//! ingested into its source scope, turns logged without waiting for
//! indexing, and pre-turn packs that carry the brain, the agent's history
//! and the team's turns once CortexDB has indexed them. Belief builds are
//! requested through `v1/beliefs/build`.

// The helpers outside `#[test]` fns fail the test by panicking, like the tests.
#![allow(clippy::expect_used)]

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tinymemory_api::{
    Consolidation, ForgetTarget, LearningKind, MemoryEngine, MemoryMeta, MetaFilter, Namespace,
    Reach, StoreItem,
};
use tinymemory_integrations::brain::brain_document;
use tinymemory_integrations::cortex::{CortexCredential, CortexEngine};
use tinymemory_integrations::documents::{ConverterChain, RawDocument};
use tinymemory_integrations::{EngineCredential, EngineSettings, build_engine};
use tinymemory_tools::{
    AgentMemory, Brain, BrainDocument, BrainSource, ContextPack, CoreScope, JobOutcome,
    MemoryLayout, PostTurn, PreTurn, RecallPolicy,
};

const DEFAULT_KEY: &str = "tinymemory-cortex-test";

/// How long an accepted write may take to be ranked by recall.
const VISIBILITY: Duration = Duration::from_secs(60);

fn live_engine() -> Option<Arc<dyn MemoryEngine>> {
    let url = std::env::var("TINYMEMORY_LIVE_CORTEXDB_URL").ok()?;
    let key = std::env::var("TINYMEMORY_TEST_CORTEX_KEY").unwrap_or_else(|_| DEFAULT_KEY.into());
    Some(Arc::new(
        CortexEngine::direct(&url, CortexCredential::api_key(key)).expect("a valid live endpoint"),
    ))
}

/// Recalls `query` until the pack contains every one of `wanted` or
/// [`VISIBILITY`] runs out.
async fn recall_until(memory: &AgentMemory, query: &str, wanted: &[&str]) -> ContextPack {
    let deadline = Instant::now() + VISIBILITY;
    loop {
        let pack = memory.recall(query).await.expect("recall");
        if wanted.iter().all(|text| pack.markdown.contains(text)) || Instant::now() >= deadline {
            return pack;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

#[tokio::test]
async fn live_an_agent_loop_runs_against_cortexdb() {
    let Some(engine) = live_engine() else {
        eprintln!("TINYMEMORY_LIVE_CORTEXDB_URL unset; skipping");
        return;
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after the epoch")
        .as_nanos();
    let layout = MemoryLayout::new(
        format!("project:live-{nanos}")
            .parse()
            .expect("a valid root"),
    )
    .expect("a valid layout");

    let handbook = RawDocument::new(
        "# Billing\n\nBilling disputes go to the finance channel within one day.\n",
    )
    .with_filename("billing.md");
    let document = brain_document(
        &ConverterChain::default(),
        &handbook,
        None,
        MemoryMeta::default(),
    )
    .await
    .expect("convert");
    let source = document.source.clone();
    let brain = Brain::new(engine.clone(), layout.clone());
    let ingested = brain.ingest(document).await.expect("ingest");

    let support = AgentMemory::new(engine.clone(), layout.clone(), "support-01").expect("agent");
    let coder = AgentMemory::new(engine.clone(), layout.clone(), "coder-42").expect("agent");

    let started = Instant::now();
    let turn = support
        .pre_turn(PreTurn::new("s-1", 0, "where do billing disputes go"))
        .await
        .expect("pre_turn");
    let pre_turn = started.elapsed();
    assert!(turn.log_error.is_none(), "{:?}", turn.log_error);
    support
        .post_turn(PostTurn::new("s-1", 1, "Billing disputes go to finance."))
        .await
        .expect("post_turn");
    coder
        .pre_turn(PreTurn::new(
            "c-1",
            0,
            "billing service migration is blocked",
        ))
        .await
        .expect("pre_turn");
    eprintln!("pre_turn took {pre_turn:?}");

    let pack = recall_until(
        &support,
        "billing disputes",
        &[
            "finance channel",
            "Billing disputes go to finance",
            "migration is blocked",
        ],
    )
    .await;
    let md = &pack.markdown;
    assert!(
        md.contains("## Brain") && md.contains("finance channel"),
        "{md}"
    );
    assert!(md.contains("## This agent's history"), "{md}");
    assert!(
        md.contains("## Team conversations") && md.contains("migration is blocked"),
        "{md}"
    );

    let built = support
        // The managed API builds on its own and hands back no job; ask for
        // one explicitly, as a refresh, so the build route is still proven.
        .run_background(match ingested.job {
            Some(job) => job,
            None => brain.build(&source).expect("build job"),
        })
        .await
        .expect("a belief build is accepted");
    eprintln!("belief build: {:?}", built.outcome);

    let forgotten = engine
        .forget(ForgetTarget::Filter(layout.holistic_filter()))
        .await
        .expect("forget");
    assert!(forgotten.forgotten >= 4, "{forgotten:?}");
}

#[tokio::test]
async fn live_core_scope_recall_and_promotion_respect_tenant_boundaries() {
    let Some(engine) = live_engine() else {
        eprintln!("TINYMEMORY_LIVE_CORTEXDB_URL unset; skipping");
        return;
    };
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after the epoch")
        .as_nanos();
    let company: Namespace = format!("project:core-{nanos}")
        .parse()
        .expect("valid namespace");
    let hive: Namespace = format!("project:core-{nanos}/team:hive")
        .parse()
        .expect("valid namespace");
    let other: Namespace = format!("project:core-{nanos}/team:other")
        .parse()
        .expect("valid namespace");
    let layout = MemoryLayout::new(hive).expect("valid layout");
    let agent = AgentMemory::new(engine.clone(), layout.clone(), "core-test")
        .expect("agent")
        .with_core(vec![CoreScope::new(company.clone(), "Company")])
        .expect("company ancestor scope");

    agent
        .promote(
            &company,
            StoreItem::learning(
                "Quasar holidays close the support desk on Friday",
                LearningKind::Fact,
                0.9,
                MemoryMeta::default(),
            ),
        )
        .await
        .expect("promote into company scope");
    let build = agent
        .core_build(&company)
        .expect("build job for configured company scope");
    agent
        .run_background(build)
        .await
        .expect("run the company-scope belief build");
    engine
        .store(StoreItem::learning(
            "Quasar holidays reveal the other tenant's private schedule",
            LearningKind::Fact,
            0.9,
            MemoryMeta {
                namespace: other.clone(),
                ..MemoryMeta::default()
            },
        ))
        .await
        .expect("store sibling fixture");

    let pack = recall_until(
        &agent,
        "Quasar holidays",
        &["Quasar holidays close the support desk on Friday"],
    )
    .await;
    assert!(
        pack.markdown
            .contains("## Company\n\n- Quasar holidays close the support desk on Friday"),
        "company core appears in recall:\n{}",
        pack.markdown
    );
    assert!(
        !pack.markdown.contains("other tenant's private schedule"),
        "sibling item is excluded:\n{}",
        pack.markdown
    );

    for namespace in [company, other] {
        let forgotten = engine
            .forget(ForgetTarget::Filter(MetaFilter {
                reach: Some(Reach::exact(namespace)),
                ..MetaFilter::default()
            }))
            .await
            .expect("clean up test data");
        assert_eq!(forgotten.forgotten, 1, "{forgotten:?}");
    }
}

/// Polls CortexDB's `v1/derivation/status` for `scope` until its last build
/// is strictly newer than its last write: the server's own scheduler rebuilt
/// after the write, with no build requested. The scope is unique to the run,
/// so its last write is this test's. Every request is bounded by the time
/// left, so the poll ends within its deadline even if the server stalls.
/// `false` if no such build happens within a few minutes.
async fn built_after_last_write(url: &str, key: &str, scope: &str) -> bool {
    use tinymemory_api::chrono::{DateTime, FixedOffset};
    let parse = |value: &serde_json::Value| -> Option<DateTime<FixedOffset>> {
        DateTime::parse_from_rfc3339(value.as_str()?).ok()
    };
    let client = reqwest::Client::new();
    let endpoint = format!("{}/v1/derivation/status", url.trim_end_matches('/'));
    let deadline = Instant::now() + Duration::from_secs(240);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let request = async {
            let response = client
                .get(&endpoint)
                .query(&[("scope", scope)])
                .bearer_auth(key)
                .send()
                .await
                .ok()?;
            response.json::<serde_json::Value>().await.ok()
        };
        let status = tokio::time::timeout(left, request).await.ok().flatten();
        if let Some(status) = &status {
            let wrote = parse(&status["last_write_at"]);
            let built = parse(&status["last_built_at"]);
            if let (Some(wrote), Some(built)) = (wrote, built)
                && built > wrote
            {
                eprintln!("derivation status: {status}");
                return true;
            }
        }
        if Instant::now() >= deadline {
            eprintln!("derivation status at the deadline: {status:?}");
            return false;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

#[tokio::test]
async fn live_an_automatic_engine_queues_no_builds_but_still_builds_on_request() {
    let Ok(url) = std::env::var("TINYMEMORY_LIVE_CORTEXDB_URL") else {
        eprintln!("TINYMEMORY_LIVE_CORTEXDB_URL unset; skipping");
        return;
    };
    let key = std::env::var("TINYMEMORY_TEST_CORTEX_KEY").unwrap_or_else(|_| DEFAULT_KEY.into());
    // A self-hosted server that runs its own layer scheduler, declared so in
    // the config, as a host would.
    let settings = EngineSettings {
        endpoint: Some(url.clone()),
        consolidation: Some(Consolidation::Automatic),
        ..EngineSettings::default()
    };
    let engine = build_engine("cortexdb", &settings, EngineCredential::Static(key.clone()))
        .expect("a valid live engine");
    assert_eq!(engine.descriptor().consolidation, Consolidation::Automatic);

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after the epoch")
        .as_nanos();
    let layout = MemoryLayout::new(
        format!("project:live-auto-{nanos}")
            .parse()
            .expect("a valid root"),
    )
    .expect("a valid layout");

    let brain = Brain::new(engine.clone(), layout.clone());
    let ingested = brain
        .ingest(BrainDocument::new(
            BrainSource::Markdown,
            "Expense reports are due on the fifth of each month.",
        ))
        .await
        .expect("ingest");
    assert_eq!(
        ingested.job, None,
        "an automatic engine gets no ingest build"
    );

    // Nothing asked for a build, yet the server rebuilds the written scope on
    // its own (the harness runs the layer scheduler: `CORTEX_V1_LAYERS_AUTO`).
    let scope = format!("app:tinymemory/project:live-auto-{nanos}/source:markdown/app:documents");
    assert!(
        built_after_last_write(&url, &key, &scope).await,
        "the server never rebuilt `{scope}` after the write on its own"
    );

    let support = AgentMemory::new(engine.clone(), layout.clone(), "support-01")
        .expect("agent")
        .with_policy(RecallPolicy {
            build_beliefs_every: Some(1),
            ..RecallPolicy::default()
        });
    let report = support
        .post_turn(PostTurn::new("auto-1", 1, "They are due on the fifth."))
        .await
        .expect("post_turn");
    assert!(report.jobs.is_empty(), "no turn build: {:?}", report.jobs);

    // An explicit refresh still reaches `v1/beliefs/build` on the real server.
    let built = support
        .run_background(brain.build(&BrainSource::Markdown).expect("build job"))
        .await
        .expect("a belief build is accepted");
    assert!(
        matches!(built.outcome, JobOutcome::Done | JobOutcome::Started),
        "{:?}",
        built.outcome
    );
    let history = support
        .run_background(support.history_build())
        .await
        .expect("a history build is accepted");
    assert!(
        matches!(history.outcome, JobOutcome::Done | JobOutcome::Started),
        "{:?}",
        history.outcome
    );

    let forgotten = engine
        .forget(ForgetTarget::Filter(layout.holistic_filter()))
        .await
        .expect("forget");
    assert!(forgotten.forgotten >= 2, "{forgotten:?}");
}
