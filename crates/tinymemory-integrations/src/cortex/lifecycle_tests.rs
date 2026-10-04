//! The agent memory lifecycle (`tinymemory_tools`) end to end over both
//! wires' doubles: the same host code against either CortexDB surface.

use std::sync::Arc;

use tinymemory_api::MemoryEngine;
use tinymemory_tools::{
    AgentMemory, Brain, BrainDocument, BrainSource, JobOutcome, MemoryLayout, PostTurn, PreTurn,
    RecallPolicy, SessionStart,
};

use crate::cortex::CortexWire;
use crate::cortex::testing::both;

#[tokio::test]
async fn an_agent_loop_runs_the_same_on_either_wire() {
    for (engine, state) in both().await {
        let wire = engine.wire();
        let engine: Arc<dyn MemoryEngine> = Arc::new(engine);
        let layout = MemoryLayout::default();

        let brain = Brain::new(engine.clone(), layout.clone());
        let ingested = brain
            .ingest(BrainDocument::new(
                BrainSource::Pdf,
                "Refunds settle within five business days.",
            ))
            .await
            .unwrap();
        let events = state.log.lock().unwrap().events.clone();
        assert!(
            events.iter().any(|e| e["scope"]
                .as_str()
                .unwrap()
                .ends_with("source:pdf/app:documents")),
            "{wire:?}: the pdf lands in its source scope"
        );

        let support = AgentMemory::new(engine.clone(), layout.clone(), "support-01")
            .unwrap()
            .with_policy(RecallPolicy {
                build_beliefs_every: Some(2),
                ..RecallPolicy::default()
            });
        let turn = support
            .pre_turn(PreTurn::new("t1", 0, "when do refunds settle"))
            .await
            .unwrap();
        assert!(turn.log_error.is_none(), "{wire:?}: {:?}", turn.log_error);
        assert!(
            turn.pack.markdown.contains("## Brain"),
            "{wire:?}: {}",
            turn.pack.markdown
        );
        assert!(turn.pack.markdown.contains("five business days"));
        let report = support
            .post_turn(PostTurn::new(
                "t1",
                1,
                "Refunds settle in five business days.",
            ))
            .await
            .unwrap();
        assert_eq!(report.jobs.len(), 1, "{wire:?}: the policy's build is due");

        if wire == CortexWire::Direct {
            let requests = state.requests();
            let unwaited = requests
                .iter()
                .filter(|r| *r == "POST /v1/experience")
                .count();
            assert_eq!(
                unwaited, 2,
                "both turns logged without waiting: {requests:?}"
            );
            assert_eq!(
                requests
                    .iter()
                    .filter(|r| *r == "POST /v1/experience?wait=indexed")
                    .count(),
                1,
                "only the brain ingest waits to be indexed"
            );
        }

        let resumed = support
            .start_session(SessionStart {
                thread_id: Some("t1".into()),
                focus: Some("refunds".into()),
            })
            .await
            .unwrap();
        assert!(
            resumed.markdown.contains("## Earlier in this thread"),
            "{wire:?}: {}",
            resumed.markdown
        );

        for job in report.jobs.into_iter().chain([ingested.job]) {
            let ran = support.run_background(job).await.unwrap();
            match wire {
                CortexWire::Direct => assert_eq!(ran.outcome, JobOutcome::Done),
                CortexWire::TinyHumans => assert_eq!(ran.outcome, JobOutcome::Scheduled),
            }
        }
        let builds = state.seen.lock().unwrap().builds.clone();
        match wire {
            CortexWire::Direct => assert_eq!(
                builds,
                [
                    serde_json::json!({ "scope": "app:tinymemory/agent:support-01/app:conversations" }),
                    serde_json::json!({ "scope": "app:tinymemory/source:pdf/app:documents" }),
                ]
            ),
            CortexWire::TinyHumans => assert!(builds.is_empty()),
        }
    }
}
