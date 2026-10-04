//! [`AgentMemory`]: one agent's memory lifecycle over any engine.
//!
//! A host calls it at fixed points of the agent loop. Every read is a
//! [`crate::recall`] over the [`MemoryLayout`]'s scopes; every write is one
//! turn at this agent's node; every slow step is a [`BackgroundJob`] handed
//! back for the host to run.
//!
//! | When | Call | Engine work |
//! | --- | --- | --- |
//! | session start or resume | [`AgentMemory::start_session`] | reads only |
//! | user turn, before the model | [`AgentMemory::pre_turn`] | logs the turn (accepted, not indexed) while fetching the pack |
//! | after the reply | [`AgentMemory::post_turn`] | logs the reply; may return a belief build |
//! | prompt truncated | [`AgentMemory::recall_for_compaction`] | an answered summary of the thread, plus related memory |
//! | any time | [`AgentMemory::recall`] | a pre-turn pack without logging |
//! | off the turn | [`AgentMemory::run_background`] | the job |
//!
//! The hot path — `pre_turn` and `post_turn` — never waits for indexing and
//! never runs a model: writes use [`WaitFor::Accepted`] and the pack is
//! ranked retrieval ([`SectionQuery::Fetch`]). Logging runs concurrently
//! with the read, and the pack never contains the turn being logged or the
//! part of the thread still in the prompt.
//!
//! A pack's sections, highest priority first (budget trimming takes from the
//! last): **Learnings**, **Brain**, **This agent's history**, and **Team
//! conversations** (other agents' turns). An item appears once, in the first
//! section that found it.
//!
//! Each turn is stored as its own one-turn conversation item carrying the
//! thread id, the turn's index, the agent id and the `conversation` source,
//! so a retried call with the same input is a replay, not a duplicate.
//!
//! # Example
//!
//! ```
//! use std::sync::Arc;
//! use tinymemory_api::conformance::ReferenceEngine;
//! use tinymemory_tools::{
//!     AgentMemory, Brain, BrainDocument, BrainSource, MemoryLayout, PostTurn, PreTurn,
//! };
//!
//! # let runtime = tokio::runtime::Builder::new_current_thread().build()?;
//! # runtime.block_on(async {
//! let engine = Arc::new(ReferenceEngine::new());
//! let layout = MemoryLayout::default();
//! Brain::new(engine.clone(), layout.clone())
//!     .ingest(BrainDocument::new(BrainSource::Markdown, "Refunds take five business days."))
//!     .await?;
//!
//! let memory = AgentMemory::new(engine, layout, "support-01")?;
//! let turn = memory
//!     .pre_turn(PreTurn::new("thread-1", 0, "How long do refunds take?"))
//!     .await?;
//! assert!(turn.pack.markdown.contains("Refunds take five business days."));
//! assert!(turn.logged.is_some());
//!
//! let report = memory
//!     .post_turn(PostTurn::new("thread-1", 1, "Five business days."))
//!     .await?;
//! assert!(!report.receipt.replayed);
//! # Ok::<(), tinymemory_api::Error>(())
//! # })?;
//! # Ok::<(), Box<dyn std::error::Error>>(())
//! ```

mod types;

use std::sync::Arc;

use futures::future::join;
use tinymemory_api::{
    ConsolidateRequest, Error, ItemKind, MemoryEngine, MemoryMeta, MetaFilter, Namespace, Reach,
    Result, Role, SourceKind, SourceRef, StoreItem, Turn, TurnRange, WriteOptions,
};

use crate::background::{BackgroundJob, BackgroundRunner, JobReport};
use crate::brain::Brain;
use crate::layout::MemoryLayout;
use crate::recall::{
    ContextPack, HolisticRecall, ScopeSection, SectionQuery, ThreadWindow, holistic_recall,
};
use crate::tools::MemoryTools;

pub use types::{
    Compaction, DEFAULT_TURN_BUDGET_TOKENS, PostTurn, PostTurnReport, PreTurn, RecallPolicy,
    SessionStart, TurnContext,
};

/// Heading of the learnings section.
pub const LEARNINGS_HEADING: &str = "Learnings";
/// Heading of the brain section.
pub const BRAIN_HEADING: &str = "Brain";
/// Heading of this agent's own past conversations.
pub const HISTORY_HEADING: &str = "This agent's history";
/// Heading of the other agents' conversations.
pub const TEAM_HEADING: &str = "Team conversations";
/// Heading of a resumed thread's own turns.
pub const THREAD_HEADING: &str = "Earlier in this thread";
/// Heading of a compaction's summary.
pub const SUMMARY_HEADING: &str = "Earlier in this conversation";

/// Title of every pack.
const PACK_TITLE: &str = "Memory";

/// Longest the gist of dropped turns used as a query may be, in characters.
const MAX_GIST_CHARS: usize = 600;

/// The question a compaction's summary answers.
const SUMMARY_QUESTION: &str = "What was discussed, decided and left open earlier in this \
     conversation?";

/// Guidance for a compaction's summary.
const SUMMARY_INSTRUCTIONS: &str = "Summarise briefly as markdown bullets: facts the user \
     gave, decisions made, and open questions. State only what the stored turns support.";

/// One agent's memory: its layout, its node, its engine and its policy.
/// Cheap to clone.
#[derive(Clone)]
pub struct AgentMemory {
    engine: Arc<dyn MemoryEngine>,
    layout: MemoryLayout,
    agent_id: String,
    node: Namespace,
    policy: RecallPolicy,
}

impl std::fmt::Debug for AgentMemory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentMemory")
            .field("engine", &self.engine.descriptor().id)
            .field("agent_id", &self.agent_id)
            .field("node", &self.node.to_string())
            .field("policy", &self.policy)
            .finish()
    }
}

impl AgentMemory {
    /// The memory of `agent_id` in `layout` on `engine`, with the default
    /// [`RecallPolicy`].
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for a blank agent id.
    pub fn new(
        engine: Arc<dyn MemoryEngine>,
        layout: MemoryLayout,
        agent_id: &str,
    ) -> Result<Self> {
        let agent_id = agent_id.trim();
        if agent_id.is_empty() {
            return Err(Error::InvalidRequest(
                "an agent's memory needs an agent id".to_string(),
            ));
        }
        Ok(Self {
            node: layout.conversations(agent_id)?,
            engine,
            layout,
            agent_id: agent_id.to_string(),
            policy: RecallPolicy::default(),
        })
    }

    /// The same memory under `policy`.
    #[must_use]
    pub fn with_policy(mut self, policy: RecallPolicy) -> Self {
        self.policy = policy;
        self
    }

    /// The agent's id.
    #[must_use]
    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    /// The node the agent's turns are stored at.
    #[must_use]
    pub fn namespace(&self) -> &Namespace {
        &self.node
    }

    /// The layout.
    #[must_use]
    pub fn layout(&self) -> &MemoryLayout {
        &self.layout
    }

    /// The policy.
    #[must_use]
    pub fn policy(&self) -> &RecallPolicy {
        &self.policy
    }

    /// The engine.
    #[must_use]
    pub fn engine(&self) -> &Arc<dyn MemoryEngine> {
        &self.engine
    }

    /// The layout's brain, on the same engine.
    #[must_use]
    pub fn brain(&self) -> Brain {
        Brain::new(self.engine.clone(), self.layout.clone())
    }

    /// A runner for the jobs this memory hands back.
    #[must_use]
    pub fn background(&self) -> BackgroundRunner {
        BackgroundRunner::new(self.engine.clone(), self.layout.clone())
    }

    /// The model-facing memory tools for this agent: writes land at its node,
    /// reads see its node and the shared root ([`Reach::of`]).
    #[must_use]
    pub fn tools(&self) -> MemoryTools {
        MemoryTools::new(self.engine.clone()).placed_at(self.node.clone())
    }

    /// Context for a session starting or resuming: a resumed thread's own
    /// turns first, then the standard sections, ranked for `focus` or newest
    /// first without one. Reads only.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for a blank thread id or a policy with no
    /// budget. Engine failures leave sections out instead (see
    /// [`crate::recall`]).
    pub async fn start_session(&self, start: SessionStart) -> Result<ContextPack> {
        let mut sections = Vec::new();
        if let Some(thread_id) = &start.thread_id {
            let thread_id = non_blank(thread_id, "thread id")?;
            sections.push(ScopeSection::latest(
                THREAD_HEADING,
                MetaFilter {
                    thread_id: Some(thread_id.to_string()),
                    ..self.layout.conversations_filter(Some(&self.agent_id))
                },
                self.policy.history_limit.max(1),
            ));
        }
        sections.extend(self.standard_sections());
        self.read(start.focus, sections).await
    }

    /// Logs the user's turn and, concurrently, recalls the context to inject
    /// before the model runs.
    ///
    /// Logging waits only for the engine to accept the turn. A failed log
    /// does not fail the call: the pack is still returned, with
    /// [`TurnContext::log_error`] set.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for a blank thread id or text. Engine
    /// failures never fail the call.
    pub async fn pre_turn(&self, turn: PreTurn) -> Result<TurnContext> {
        let thread_id = non_blank(&turn.thread_id, "thread id")?;
        let text = non_blank(&turn.user_text, "user text")?;
        let item = self.turn_item(
            thread_id,
            turn.turn_index,
            Turn {
                at: turn.at,
                ..Turn::new(Role::User, text)
            },
        );
        let id = tinymemory_api::ItemId::new(item.fingerprint());
        let window = ThreadWindow {
            thread_id: thread_id.to_string(),
            from_turn: turn.in_prompt_from,
        };
        let mut request = self.request(Some(text.to_string()), self.standard_sections());
        request.exclude_ids = vec![id];
        request.exclude_thread = Some(window);
        let (logged, pack) = join(
            self.engine.store_with(item, WriteOptions::accepted()),
            holistic_recall(self.engine.as_ref(), &request),
        )
        .await;
        let pack = pack?;
        let (logged, log_error) = match logged {
            Ok(receipt) => (Some(receipt), None),
            Err(error) => {
                log::warn!(
                    "[lifecycle] user turn not logged agent={} thread={thread_id} error={error}",
                    self.agent_id
                );
                (None, Some(error.to_string()))
            }
        };
        Ok(TurnContext {
            pack,
            logged,
            log_error,
        })
    }

    /// Logs the assistant's reply, and returns the belief build the policy
    /// asks for at this turn, if any.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for a blank thread id or text, and the
    /// engine's failure to accept the turn.
    pub async fn post_turn(&self, turn: PostTurn) -> Result<PostTurnReport> {
        let thread_id = non_blank(&turn.thread_id, "thread id")?;
        let text = non_blank(&turn.assistant_text, "assistant text")?;
        let item = self.turn_item(
            thread_id,
            turn.turn_index,
            Turn {
                at: turn.at,
                tool_calls: turn.tool_calls.clone(),
                ..Turn::new(Role::Assistant, text)
            },
        );
        let receipt = self
            .engine
            .store_with(item, WriteOptions::accepted())
            .await?;
        let due = self
            .policy
            .build_beliefs_every
            .is_some_and(|every| every > 0 && (turn.turn_index + 1).is_multiple_of(every));
        let jobs = if due {
            vec![self.history_build()]
        } else {
            Vec::new()
        };
        Ok(PostTurnReport { receipt, jobs })
    }

    /// Context to carry across a compaction: an answered summary of the
    /// thread so far (falling back to its most relevant turns when the engine
    /// cannot answer), then the standard sections, ranked for `focus` or the
    /// dropped turns' gist.
    ///
    /// Runs a model on engines that answer with one; call it off the hot
    /// path.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for a blank thread id. Engine failures leave
    /// sections out instead.
    pub async fn recall_for_compaction(&self, compaction: Compaction) -> Result<ContextPack> {
        let thread_id = non_blank(&compaction.thread_id, "thread id")?;
        let gist = gist(&compaction.dropped);
        let query = compaction
            .focus
            .filter(|focus| !focus.trim().is_empty())
            .or(gist);
        let summary = ScopeSection {
            heading: SUMMARY_HEADING.to_string(),
            filter: MetaFilter {
                thread_id: Some(thread_id.to_string()),
                ..self.layout.conversations_filter(Some(&self.agent_id))
            },
            limit: self.policy.history_limit.max(1),
            query: SectionQuery::Answer {
                question: SUMMARY_QUESTION.to_string(),
                instructions: Some(SUMMARY_INSTRUCTIONS.to_string()),
                fallback_to_fetch: true,
            },
        };
        let mut sections = vec![summary];
        sections.extend(self.standard_sections());
        self.read(query, sections).await
    }

    /// A pre-turn pack for `query`, without logging anything: for a tool, a
    /// sub-agent, or a mid-turn refresh.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for a policy with no budget.
    pub async fn recall(&self, query: &str) -> Result<ContextPack> {
        let query = (!query.trim().is_empty()).then(|| query.to_string());
        self.read(query, self.standard_sections()).await
    }

    /// Runs one job this memory (or its brain) handed back.
    ///
    /// # Errors
    ///
    /// As [`BackgroundRunner::run`].
    pub async fn run_background(&self, job: BackgroundJob) -> Result<JobReport> {
        self.background().run(job).await
    }

    /// A belief build of this agent's conversations.
    #[must_use]
    pub fn history_build(&self) -> BackgroundJob {
        BackgroundJob::BuildBeliefs {
            request: ConsolidateRequest::new(Reach::exact(self.node.clone()))
                .kinds([ItemKind::Conversation]),
        }
    }

    /// Learnings, brain, this agent's history, then the team's, each filled
    /// by fetch; a zero limit leaves its section out.
    fn standard_sections(&self) -> Vec<ScopeSection> {
        let policy = &self.policy;
        [
            (
                LEARNINGS_HEADING,
                self.layout.learnings_filter(),
                policy.learnings_limit,
            ),
            (
                BRAIN_HEADING,
                self.layout.brain_filter(None),
                policy.brain_limit,
            ),
            (
                HISTORY_HEADING,
                self.layout.conversations_filter(Some(&self.agent_id)),
                policy.history_limit,
            ),
            (
                TEAM_HEADING,
                self.layout.conversations_filter(None),
                policy.team_limit,
            ),
        ]
        .into_iter()
        .filter(|(_, _, limit)| *limit > 0)
        .map(|(heading, filter, limit)| ScopeSection::fetch(heading, filter, limit))
        .collect()
    }

    fn request(&self, query: Option<String>, sections: Vec<ScopeSection>) -> HolisticRecall {
        HolisticRecall {
            budget_tokens: self.policy.budget_tokens,
            title: PACK_TITLE.to_string(),
            ..HolisticRecall::new(query, sections)
        }
    }

    async fn read(
        &self,
        query: Option<String>,
        sections: Vec<ScopeSection>,
    ) -> Result<ContextPack> {
        holistic_recall(self.engine.as_ref(), &self.request(query, sections)).await
    }

    /// One turn of `thread_id` as a one-turn conversation at this agent's
    /// node.
    fn turn_item(&self, thread_id: &str, index: u32, turn: Turn) -> StoreItem {
        StoreItem::Conversation {
            meta: MemoryMeta {
                namespace: self.node.clone(),
                thread_id: Some(thread_id.to_string()),
                turns: Some(TurnRange {
                    first: index,
                    last: index,
                }),
                agent_id: Some(self.agent_id.clone()),
                source: SourceRef {
                    kind: SourceKind::Conversation,
                    id: Some(thread_id.to_string()),
                },
                observed_at: turn.at,
                ..MemoryMeta::default()
            },
            turns: vec![turn],
        }
    }
}

/// `value` trimmed, refused when blank.
fn non_blank<'a>(value: &'a str, what: &str) -> Result<&'a str> {
    let value = value.trim();
    if value.is_empty() {
        Err(Error::InvalidRequest(format!(
            "the {what} must not be blank"
        )))
    } else {
        Ok(value)
    }
}

/// The dropped turns' text, newest last, cut to [`MAX_GIST_CHARS`] from the
/// end (the most recent turns matter most); `None` when they say nothing.
fn gist(turns: &[Turn]) -> Option<String> {
    let joined = turns
        .iter()
        .map(|turn| turn.text.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if joined.is_empty() {
        return None;
    }
    let count = joined.chars().count();
    Some(
        joined
            .chars()
            .skip(count.saturating_sub(MAX_GIST_CHARS))
            .collect(),
    )
}

#[cfg(test)]
#[path = "mod_tests.rs"]
mod tests;
