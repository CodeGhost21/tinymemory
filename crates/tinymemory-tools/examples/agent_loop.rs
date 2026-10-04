//! A whole agent memory lifecycle, offline, against the in-memory reference
//! engine: brain ingestion, two agents' turns with pre-turn context
//! injection, cross-agent recall, a session resume, compaction, and the
//! background belief builds the turns hand back.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p tinymemory-tools --example agent_loop
//! ```
//!
//! Swap `ReferenceEngine::new()` for any other `MemoryEngine` (the CortexDB
//! one is in `tinymemory-integrations`, see its `cortex_agent` example) and
//! nothing else changes.

use std::sync::Arc;

use tinymemory_api::conformance::ReferenceEngine;
use tinymemory_api::{MemoryEngine, Role, Turn};
use tinymemory_tools::{
    AgentMemory, BackgroundJob, Brain, BrainDocument, BrainSource, Compaction, MemoryLayout,
    PostTurn, PreTurn, RecallPolicy, SessionStart,
};

/// Stands in for the model: answers from the injected context.
fn generate(context: &str, user: &str) -> String {
    let grounded = context
        .lines()
        .filter(|line| line.starts_with("- "))
        .map(|line| line.trim_start_matches("- "))
        .find(|line| {
            user.split_whitespace()
                .filter(|word| word.len() > 4)
                .any(|word| line.to_lowercase().contains(&word.to_lowercase()))
        });
    match grounded {
        Some(fact) => format!("From memory: {fact}"),
        None => "I don't have that in memory yet.".to_string(),
    }
}

/// One turn of the agent loop: pre-turn context, generation, post-turn log.
async fn turn(
    memory: &AgentMemory,
    thread: &str,
    index: u32,
    user: &str,
    jobs: &mut Vec<BackgroundJob>,
) -> Result<String, Box<dyn std::error::Error>> {
    let context = memory
        .pre_turn(PreTurn::new(thread, index, user))
        .await?;
    let reply = generate(&context.pack.markdown, user);
    let report = memory
        .post_turn(PostTurn::new(thread, index + 1, reply.clone()))
        .await?;
    jobs.extend(report.jobs);
    println!(
        "[{}] user: {user}\n[{}] context: {} tokens, {} refs\n[{}] reply: {reply}\n",
        memory.agent_id(),
        memory.agent_id(),
        context.pack.tokens,
        context.pack.refs.len(),
        memory.agent_id(),
    );
    Ok(reply)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let engine: Arc<dyn MemoryEngine> = Arc::new(ReferenceEngine::new());
    let layout = MemoryLayout::default();
    let mut jobs: Vec<BackgroundJob> = Vec::new();

    // 1. The brain: company documents, global to every agent, by source.
    let brain = Brain::new(engine.clone(), layout.clone());
    let batch = brain
        .ingest_many(vec![
            BrainDocument::new(
                BrainSource::Markdown,
                "Every support ticket gets a first reply within four hours.",
            )
            .titled("onboarding.md"),
            BrainDocument::new(
                BrainSource::Notion,
                "Refunds settle within five business days of approval.",
            )
            .titled("Refund policy"),
        ])
        .await?;
    println!(
        "brain: {} documents stored, {} belief builds queued\n",
        batch.receipts.len(),
        batch.jobs.len()
    );
    jobs.extend(batch.jobs);

    // 2. Two agents, each logging to its own scope and reading the whole tree.
    let policy = RecallPolicy {
        build_beliefs_every: Some(4),
        ..RecallPolicy::default()
    };
    let support = AgentMemory::new(engine.clone(), layout.clone(), "support-01")?
        .with_policy(policy.clone());
    let coder = AgentMemory::new(engine.clone(), layout.clone(), "coder-42")?.with_policy(policy);

    turn(&support, "s-1", 0, "How long do refunds take to settle?", &mut jobs).await?;
    turn(&coder, "c-1", 0, "The refund webhook deploy failed on Friday.", &mut jobs).await?;
    turn(&support, "s-1", 2, "My customer says the refund webhook is broken.", &mut jobs).await?;

    // 3. Cross-agent recall: the coder's turn shows under the team.
    let team = support.recall("refund webhook").await?;
    assert!(team.markdown.contains("## Team conversations"));
    println!("support's view of the team:\n{}\n", team.markdown);

    // 4. A new session resumes the thread from memory alone.
    let resumed = support
        .start_session(SessionStart {
            thread_id: Some("s-1".into()),
            focus: None,
        })
        .await?;
    println!("session resume:\n{}\n", resumed.markdown);

    // 5. The prompt overflows: carry the dropped turns forward as a summary.
    let carried = support
        .recall_for_compaction(Compaction {
            thread_id: "s-1".into(),
            dropped: vec![
                Turn::new(Role::User, "How long do refunds take to settle?"),
                Turn::new(Role::User, "My customer says the refund webhook is broken."),
            ],
            focus: Some("refund webhook".into()),
        })
        .await?;
    println!("compaction carry-over:\n{}\n", carried.markdown);

    // 6. Off the hot path: run every queued job.
    for job in jobs {
        let report = support.run_background(job).await?;
        println!("background {}: {:?}", report.job, report.outcome);
    }
    let after = support.recall("refunds settle").await?;
    assert!(after.markdown.contains("## Learnings"));
    println!("\nafter the belief builds:\n{}", after.markdown);
    Ok(())
}
