//! The agent lifecycle against the reference engine and a write-broken one.

use async_trait::async_trait;
use tinymemory_api::conformance::ReferenceEngine;
use tinymemory_api::{
    EngineDescriptor, EngineHealth, FetchPage, FetchRequest, ForgetReport, ForgetTarget,
    LearningKind, ListPage, ListRequest, RecallAnswer, RecallRequest, StoreReceipt, ToolCallRef,
};

use super::*;
use crate::brain::BrainDocument;
use crate::layout::BrainSource;

fn memory(engine: &Arc<ReferenceEngine>, agent: &str) -> AgentMemory {
    AgentMemory::new(engine.clone(), MemoryLayout::default(), agent).unwrap()
}

async fn with_brain() -> Arc<ReferenceEngine> {
    let engine = Arc::new(ReferenceEngine::new());
    let brain = Brain::new(engine.clone(), MemoryLayout::default());
    brain
        .ingest(BrainDocument::new(
            BrainSource::Pdf,
            "Refunds take five business days.",
        ))
        .await
        .unwrap();
    engine
        .store(StoreItem::learning(
            "Customers want refund updates by email",
            LearningKind::Fact,
            0.9,
            MemoryMeta::default(),
        ))
        .await
        .unwrap();
    engine
}

#[tokio::test]
async fn pre_turn_logs_the_turn_and_recalls_without_it() {
    let engine = with_brain().await;
    let support = memory(&engine, "support-01");
    let context = support
        .pre_turn(PreTurn::new("t1", 0, "how long do refunds take"))
        .await
        .unwrap();
    let md = &context.pack.markdown;
    assert!(md.starts_with("# Memory\n"), "{md}");
    assert!(md.contains("## Learnings\n\n- Customers want refund updates by email"));
    assert!(md.contains("## Brain\n\n- Refunds take five business days."));
    assert!(
        !md.contains("how long do refunds take"),
        "the live turn is left out"
    );
    let receipt = context.logged.unwrap();
    assert!(context.log_error.is_none());

    let listed = engine
        .list(ListRequest::new(
            MetaFilter::kinds([ItemKind::Conversation]),
            10,
        ))
        .await
        .unwrap();
    let logged = &listed.items[0];
    assert_eq!(logged.id, receipt.id);
    assert_eq!(logged.meta.namespace, Namespace::agent("support-01"));
    assert_eq!(logged.meta.agent_id.as_deref(), Some("support-01"));
    assert_eq!(logged.meta.thread_id.as_deref(), Some("t1"));
    assert_eq!(logged.meta.turns, Some(TurnRange { first: 0, last: 0 }));
    assert_eq!(logged.meta.source.kind, SourceKind::Conversation);
}

#[tokio::test]
async fn the_thread_in_the_prompt_is_left_out_until_it_is_compacted_away() {
    let engine = with_brain().await;
    let support = memory(&engine, "support-01");
    support
        .pre_turn(PreTurn::new(
            "t1",
            0,
            "my order number is 4417 for the refund",
        ))
        .await
        .unwrap();
    support
        .post_turn(PostTurn::new(
            "t1",
            1,
            "Thanks, refund for order 4417 noted.",
        ))
        .await
        .unwrap();

    let in_prompt = support
        .pre_turn(PreTurn::new("t1", 2, "what was my refund order number"))
        .await
        .unwrap();
    assert!(!in_prompt.pack.markdown.contains("4417"));

    let compacted = support
        .pre_turn(PreTurn {
            in_prompt_from: 2,
            ..PreTurn::new("t1", 3, "what was my refund order number again")
        })
        .await
        .unwrap();
    assert!(
        compacted.pack.markdown.contains("## This agent's history"),
        "{}",
        compacted.pack.markdown
    );
    assert!(compacted.pack.markdown.contains("4417"));
}

#[tokio::test]
async fn other_agents_turns_appear_once_under_the_team() {
    let engine = with_brain().await;
    let coder = memory(&engine, "coder-42");
    let support = memory(&engine, "support-01");
    coder
        .pre_turn(PreTurn::new("c1", 0, "the refund service deploy failed"))
        .await
        .unwrap();
    support
        .pre_turn(PreTurn::new("s1", 0, "refund delayed for a customer"))
        .await
        .unwrap();

    let pack = support.recall("refund").await.unwrap();
    let md = &pack.markdown;
    let history = md.find("## This agent's history").unwrap();
    let team = md.find("## Team conversations").unwrap();
    assert!(md[history..team].contains("refund delayed"));
    assert!(md[team..].contains("deploy failed"));
    assert_eq!(md.matches("refund delayed").count(), 1, "shown once: {md}");
}

#[tokio::test]
async fn post_turn_asks_for_a_belief_build_on_the_policy_s_cadence() {
    let engine = Arc::new(ReferenceEngine::new());
    let support = memory(&engine, "support-01").with_policy(RecallPolicy {
        build_beliefs_every: Some(2),
        ..RecallPolicy::default()
    });
    let first = support
        .post_turn(PostTurn {
            tool_calls: vec![ToolCallRef {
                name: "lookup_order".into(),
                id: Some("call-1".into()),
            }],
            ..PostTurn::new("t1", 0, "Looking that up.")
        })
        .await
        .unwrap();
    assert!(first.jobs.is_empty());
    let second = support
        .post_turn(PostTurn::new("t1", 1, "Found it."))
        .await
        .unwrap();
    assert_eq!(second.jobs, [support.history_build()]);

    let listed = engine
        .list(ListRequest::new(MetaFilter::default(), 10))
        .await
        .unwrap();
    assert!(
        listed
            .items
            .iter()
            .any(|hit| hit.text.contains("lookup_order"))
    );

    let never = memory(&engine, "quiet").with_policy(RecallPolicy {
        build_beliefs_every: None,
        ..RecallPolicy::default()
    });
    for index in 0..4 {
        let report = never
            .post_turn(PostTurn::new("t2", index, format!("reply {index}")))
            .await
            .unwrap();
        assert!(report.jobs.is_empty());
    }
}

#[tokio::test]
async fn a_retried_turn_is_a_replay() {
    let engine = Arc::new(ReferenceEngine::new());
    let support = memory(&engine, "support-01");
    let first = support
        .post_turn(PostTurn::new("t1", 1, "Five days."))
        .await
        .unwrap();
    let again = support
        .post_turn(PostTurn::new("t1", 1, "Five days."))
        .await
        .unwrap();
    assert_eq!(first.receipt.id, again.receipt.id);
    assert!(again.receipt.replayed);
}

#[tokio::test]
async fn a_belief_build_turns_history_into_learnings() {
    let engine = Arc::new(ReferenceEngine::new());
    let support = memory(&engine, "support-01");
    support
        .pre_turn(PreTurn::new(
            "t1",
            0,
            "I prefer refunds to my original card.",
        ))
        .await
        .unwrap();
    let report = support
        .run_background(support.history_build())
        .await
        .unwrap();
    assert_eq!(report.outcome, crate::background::JobOutcome::Done);
    let pack = support.recall("refunds card").await.unwrap();
    let learnings = pack.markdown.find("## Learnings").unwrap();
    assert!(pack.markdown[learnings..].contains("I prefer refunds to my original card."));
}

#[tokio::test]
async fn start_session_resumes_the_thread_first() {
    let engine = with_brain().await;
    let support = memory(&engine, "support-01");
    support
        .pre_turn(PreTurn::new("t1", 0, "order 4417 refund"))
        .await
        .unwrap();
    support
        .post_turn(PostTurn::new("t1", 1, "Refund for 4417 is on its way."))
        .await
        .unwrap();

    let resumed = support
        .start_session(SessionStart {
            thread_id: Some("t1".into()),
            focus: None,
        })
        .await
        .unwrap();
    let md = &resumed.markdown;
    let thread = md.find("## Earlier in this thread").unwrap();
    assert!(thread < md.find("## Learnings").unwrap());
    assert!(md[thread..].starts_with("## Earlier in this thread\n\n- assistant: Refund for 4417"));

    let fresh = support
        .start_session(SessionStart::default())
        .await
        .unwrap();
    assert!(!fresh.markdown.contains("## Earlier in this thread"));
    assert!(fresh.markdown.contains("## Brain"));
}

#[tokio::test]
async fn compaction_summarises_the_thread_then_adds_related_memory() {
    let engine = with_brain().await;
    let support = memory(&engine, "support-01");
    support
        .pre_turn(PreTurn::new("t1", 0, "my refund for order 4417 is late"))
        .await
        .unwrap();
    let pack = support
        .recall_for_compaction(Compaction {
            thread_id: "t1".into(),
            dropped: vec![Turn::new(Role::User, "my refund for order 4417 is late")],
            focus: None,
        })
        .await
        .unwrap();
    let md = &pack.markdown;
    let summary = md.find("## Earlier in this conversation").unwrap();
    assert!(md[summary..].contains("4417"));
    assert!(pack.sections[0].answer.is_some());
    assert!(md.contains("## Brain"));
}

#[tokio::test]
async fn blank_inputs_are_refused() {
    let engine = Arc::new(ReferenceEngine::new());
    assert!(AgentMemory::new(engine.clone(), MemoryLayout::default(), " ").is_err());
    let support = memory(&engine, "support-01");
    let refusals = [
        support.pre_turn(PreTurn::new(" ", 0, "hi")).await.err(),
        support.pre_turn(PreTurn::new("t", 0, " ")).await.err(),
        support.post_turn(PostTurn::new("t", 0, "")).await.err(),
        support
            .start_session(SessionStart {
                thread_id: Some(String::new()),
                focus: None,
            })
            .await
            .err(),
        support
            .recall_for_compaction(Compaction {
                thread_id: " ".into(),
                dropped: Vec::new(),
                focus: None,
            })
            .await
            .err(),
    ];
    for refusal in refusals {
        assert!(
            matches!(refusal, Some(Error::InvalidRequest(_))),
            "{refusal:?}"
        );
    }
    assert!(engine.is_empty());
}

/// The reference engine refusing every write.
struct ReadOnly(ReferenceEngine);

#[async_trait]
impl MemoryEngine for ReadOnly {
    fn descriptor(&self) -> &EngineDescriptor {
        self.0.descriptor()
    }
    async fn health(&self) -> EngineHealth {
        EngineHealth::Ok
    }
    async fn recall(&self, req: RecallRequest) -> Result<RecallAnswer> {
        self.0.recall(req).await
    }
    async fn fetch(&self, req: FetchRequest) -> Result<FetchPage> {
        self.0.fetch(req).await
    }
    async fn store(&self, _item: StoreItem) -> Result<StoreReceipt> {
        Err(Error::Unavailable("writes are down".into()))
    }
    async fn forget(&self, target: ForgetTarget) -> Result<ForgetReport> {
        self.0.forget(target).await
    }
    async fn list(&self, req: ListRequest) -> Result<ListPage> {
        self.0.list(req).await
    }
}

#[tokio::test]
async fn a_failed_log_still_returns_the_pack() {
    let inner = ReferenceEngine::new();
    inner
        .store(StoreItem::document(
            "Refunds take five days.",
            MemoryMeta::default(),
        ))
        .await
        .unwrap();
    let support =
        AgentMemory::new(Arc::new(ReadOnly(inner)), MemoryLayout::default(), "s").unwrap();
    let context = support
        .pre_turn(PreTurn::new("t1", 0, "refunds"))
        .await
        .unwrap();
    assert!(context.logged.is_none());
    assert!(context.log_error.unwrap().contains("writes are down"));
    assert!(context.pack.markdown.contains("Refunds take five days."));

    let error = support
        .post_turn(PostTurn::new("t1", 1, "Five days."))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::Unavailable(_)));
}

#[test]
fn the_gist_samples_every_dropped_turn() {
    assert_eq!(gist(&[]), None);
    assert_eq!(gist(&[Turn::new(Role::User, "  ")]), None);
    let long = vec![
        Turn::new(Role::User, "The offsite is in Porto."),
        Turn::new(Role::Assistant, "a".repeat(MAX_GIST_CHARS)),
        Turn::new(Role::User, "the end"),
    ];
    let gist = gist(&long).unwrap();
    assert!(gist.chars().count() <= MAX_GIST_CHARS);
    assert!(gist.starts_with("The offsite is in Porto."), "{gist}");
    assert!(gist.ends_with("the end"), "{gist}");

    let many: Vec<Turn> = (0..100)
        .map(|n| Turn::new(Role::User, format!("turn {n} {}", "x".repeat(80))))
        .collect();
    let gist = super::gist(&many).unwrap();
    assert_eq!(gist.chars().count(), MAX_GIST_CHARS);
    assert!(gist.starts_with("turn 0 "), "the oldest turns come first");
}

#[test]
fn a_zero_limit_leaves_its_section_out() {
    let engine = Arc::new(ReferenceEngine::new());
    let support = memory(&engine, "s").with_policy(RecallPolicy {
        team_limit: 0,
        ..RecallPolicy::default()
    });
    let headings: Vec<String> = support
        .standard_sections()
        .into_iter()
        .map(|section| section.heading)
        .collect();
    assert_eq!(
        headings,
        [LEARNINGS_HEADING, BRAIN_HEADING, HISTORY_HEADING]
    );
}

/// A hive below a company: a company fact, a root fact, a sibling tenant's
/// secret and the hive's own learning, with `a` in the hive.
async fn company() -> (Arc<ReferenceEngine>, AgentMemory) {
    let engine = Arc::new(ReferenceEngine::new());
    for (at, text) in [
        ("ws:acme", "Acme closes for the holidays on Friday"),
        ("root", "Every agent answers in English"),
        (
            "ws:acme/team:other",
            "The other team closes for the holidays on Monday",
        ),
        ("ws:acme/team:hive", "The hive ships on Mondays"),
    ] {
        engine
            .store(StoreItem::learning(
                text,
                LearningKind::Fact,
                0.9,
                MemoryMeta {
                    namespace: at.parse().unwrap(),
                    ..MemoryMeta::default()
                },
            ))
            .await
            .unwrap();
    }
    let layout = MemoryLayout::new("ws:acme/team:hive".parse().unwrap()).unwrap();
    let memory = AgentMemory::new(engine.clone(), layout, "a").unwrap();
    (engine, memory)
}

fn acme() -> Namespace {
    "ws:acme".parse().unwrap()
}

#[tokio::test]
async fn a_core_scope_adds_its_section_after_learnings() {
    let (_, memory) = company().await;
    let without = memory.recall("").await.unwrap().markdown;
    assert_eq!(without, "# Memory\n\n## Learnings\n\n- The hive ships on Mondays");
    assert!(!without.contains("holidays"), "{without}");

    let memory = memory
        .with_core(vec![CoreScope::new(acme(), "Company")])
        .unwrap();
    let md = memory.recall("").await.unwrap().markdown;
    let learnings = md.find("## Learnings").unwrap();
    let company = md
        .find("## Company\n\n- Acme closes for the holidays on Friday")
        .unwrap();
    assert!(learnings < company, "{md}");
    assert!(md.contains("The hive ships on Mondays"));
}

#[tokio::test]
async fn a_core_scope_never_reads_a_sibling_tenant() {
    let (_, memory) = company().await;
    let memory = memory
        .with_core(vec![
            CoreScope::new(Namespace::ROOT, "Core"),
            CoreScope::new(acme(), "Company"),
        ])
        .unwrap();
    let md = memory.recall("").await.unwrap().markdown;
    assert!(
        md.contains("## Core\n\n- Every agent answers in English"),
        "{md}"
    );
    assert!(md.contains("Acme closes"));
    assert!(!md.contains("Kestrel"), "{md}");
}

#[tokio::test]
async fn with_core_replaces_the_set_per_call() {
    let (_, memory) = company().await;
    let without = memory.recall("").await.unwrap().markdown;
    let memory = memory
        .with_core(vec![CoreScope::new(acme(), "Company")])
        .unwrap();
    let override_ = memory
        .clone()
        .with_core(vec![CoreScope::new(Namespace::ROOT, "Core")])
        .unwrap();
    let md = override_.recall("").await.unwrap().markdown;
    assert!(md.contains("## Core") && !md.contains("## Company"), "{md}");
    assert_eq!(memory.core()[0].at, acme(), "the original keeps its set");

    let dropped = memory.clone().with_core(Vec::new()).unwrap();
    assert!(dropped.core().is_empty());
    assert_eq!(dropped.recall("").await.unwrap().markdown, without);
}

#[tokio::test]
async fn rejects_a_core_scope_outside_the_ancestors() {
    let (_, memory) = company().await;
    for at in [
        "ws:acme/team:hive",
        "ws:acme/team:other",
        "ws:acme/team:hive/agent:a",
    ] {
        let refused = memory
            .clone()
            .with_core(vec![CoreScope::new(at.parse().unwrap(), "Shared")]);
        assert!(matches!(refused, Err(Error::InvalidRequest(_))), "{at}");
    }
}

#[tokio::test]
async fn rejects_a_duplicate_core_node_or_a_blank_heading() {
    let (_, memory) = company().await;
    let twice = memory.clone().with_core(vec![
        CoreScope::new(acme(), "Company"),
        CoreScope::new(acme(), "Again"),
    ]);
    assert!(matches!(twice, Err(Error::InvalidRequest(_))));
    let blank = memory.with_core(vec![CoreScope::new(acme(), "  ")]);
    assert!(matches!(blank, Err(Error::InvalidRequest(_))));
}

#[tokio::test]
async fn a_zero_limit_core_scope_is_left_out() {
    let (_, memory) = company().await;
    let memory = memory
        .with_core(vec![CoreScope::new(acme(), "Company").limit(0)])
        .unwrap();
    let md = memory.recall("").await.unwrap().markdown;
    assert!(!md.contains("## Company"), "{md}");
}

#[tokio::test]
async fn promote_writes_at_the_core_node() {
    let (engine, memory) = company().await;
    let memory = memory
        .with_core(vec![CoreScope::new(acme(), "Company")])
        .unwrap();
    let receipt = memory
        .promote(
            &acme(),
            StoreItem::document("The expense limit is 500 euros", MemoryMeta::default()),
        )
        .await
        .unwrap();
    let stored = engine
        .list(ListRequest::new(
            MetaFilter::kinds([ItemKind::Document]),
            10,
        ))
        .await
        .unwrap();
    assert_eq!(stored.items[0].id, receipt.id);
    assert_eq!(stored.items[0].meta.namespace, acme());

    let other = AgentMemory::new(
        engine.clone(),
        MemoryLayout::new("ws:acme/team:hive".parse().unwrap()).unwrap(),
        "b",
    )
    .unwrap()
    .with_core(vec![CoreScope::new(acme(), "Company")])
    .unwrap();
    let md = other.recall("expense limit").await.unwrap().markdown;
    assert!(md.contains("The expense limit is 500 euros"), "{md}");
}

#[tokio::test]
async fn promote_rejects_a_conversation_and_an_unconfigured_node() {
    let (_, memory) = company().await;
    let learning = StoreItem::learning("x", LearningKind::Fact, 0.5, MemoryMeta::default());
    let unconfigured = memory.promote(&acme(), learning.clone()).await;
    assert!(matches!(unconfigured, Err(Error::InvalidRequest(_))));

    let memory = memory
        .with_core(vec![CoreScope::new(acme(), "Company")])
        .unwrap();
    let conversation = StoreItem::Conversation {
        meta: MemoryMeta::default(),
        turns: vec![Turn::new(Role::User, "hello")],
    };
    let refused = memory.promote(&acme(), conversation).await;
    assert!(matches!(refused, Err(Error::InvalidRequest(_))));

    let learning_only = memory
        .with_core(vec![
            CoreScope::new(acme(), "Company").kinds([ItemKind::Learning]),
        ])
        .unwrap();
    let document = StoreItem::document("x", MemoryMeta::default());
    assert!(matches!(
        learning_only.promote(&acme(), document).await,
        Err(Error::InvalidRequest(_))
    ));
}

#[tokio::test]
async fn core_build_consolidates_exactly_the_core_node() {
    let (_, memory) = company().await;
    assert!(matches!(
        memory.core_build(&acme()),
        Err(Error::InvalidRequest(_))
    ));
    let memory = memory
        .with_core(vec![CoreScope::new(acme(), "Company")])
        .unwrap();
    let BackgroundJob::BuildBeliefs { request } = memory.core_build(&acme()).unwrap() else {
        panic!("a core build is a belief build");
    };
    assert_eq!(request.reach, Reach::exact(acme()));
    assert!(request.kinds.is_empty());
}
