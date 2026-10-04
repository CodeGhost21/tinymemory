//! The inputs and outputs of each lifecycle step, and the recall policy.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tinymemory_api::{StoreReceipt, ToolCallRef, Turn};

use crate::background::BackgroundJob;
use crate::recall::ContextPack;

/// Default token budget of a turn's context pack.
pub const DEFAULT_TURN_BUDGET_TOKENS: usize = 1_200;

/// How an [`crate::AgentMemory`] fills its packs and when it asks for
/// belief builds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RecallPolicy {
    /// The most tokens a pack's block may take.
    pub budget_tokens: usize,
    /// The most learnings a pack shows.
    pub learnings_limit: usize,
    /// The most brain documents a pack shows.
    pub brain_limit: usize,
    /// The most of this agent's past turns a pack shows.
    pub history_limit: usize,
    /// The most other agents' turns a pack shows; `0` leaves the team's
    /// conversations out.
    pub team_limit: usize,
    /// Ask for a belief build of this agent's conversations after every this
    /// many turns (by `turn_index + 1`); `None` never asks.
    pub build_beliefs_every: Option<u32>,
}

impl Default for RecallPolicy {
    fn default() -> Self {
        Self {
            budget_tokens: DEFAULT_TURN_BUDGET_TOKENS,
            learnings_limit: 8,
            brain_limit: 6,
            history_limit: 6,
            team_limit: 3,
            build_beliefs_every: Some(10),
        }
    }
}

/// A session starting or resuming.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionStart {
    /// The thread being resumed, if any: its own past turns come first in
    /// this agent's history.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread_id: Option<String>,
    /// What the session is about, when known: ranks every section. Without
    /// one, each section shows its newest items.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
}

/// A user turn about to be answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreTurn {
    /// The conversation thread.
    pub thread_id: String,
    /// This turn's position in the thread, from `0`.
    pub turn_index: u32,
    /// What the user said.
    pub user_text: String,
    /// The first turn of this thread still in the host's prompt; turns from
    /// here on are left out of the pack, since the model already sees them.
    /// `0` (the default) leaves the whole thread out.
    #[serde(default)]
    pub in_prompt_from: u32,
    /// When the user spoke, if the host knows: orders history newest first.
    /// Leave it out to keep a retried turn a replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<DateTime<Utc>>,
}

impl PreTurn {
    /// A turn of `thread_id` at `turn_index` saying `user_text`, with the
    /// whole thread in the prompt.
    #[must_use]
    pub fn new(thread_id: impl Into<String>, turn_index: u32, user_text: impl Into<String>) -> Self {
        Self {
            thread_id: thread_id.into(),
            turn_index,
            user_text: user_text.into(),
            in_prompt_from: 0,
            at: None,
        }
    }
}

/// What [`crate::AgentMemory::pre_turn`] produced.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TurnContext {
    /// The context to inject before the model runs.
    pub pack: ContextPack,
    /// The logged user turn's receipt; `None` when logging failed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub logged: Option<StoreReceipt>,
    /// Why logging failed, when it did. The pack is still good.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_error: Option<String>,
}

/// A reply the model just gave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PostTurn {
    /// The conversation thread.
    pub thread_id: String,
    /// This reply's position in the thread.
    pub turn_index: u32,
    /// What the assistant said.
    pub assistant_text: String,
    /// The tool calls the reply made.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCallRef>,
    /// When the reply was given, if the host knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<DateTime<Utc>>,
}

impl PostTurn {
    /// A reply of `thread_id` at `turn_index` saying `assistant_text`, with
    /// no tool calls.
    #[must_use]
    pub fn new(
        thread_id: impl Into<String>,
        turn_index: u32,
        assistant_text: impl Into<String>,
    ) -> Self {
        Self {
            thread_id: thread_id.into(),
            turn_index,
            assistant_text: assistant_text.into(),
            tool_calls: Vec::new(),
            at: None,
        }
    }
}

/// What [`crate::AgentMemory::post_turn`] did.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PostTurnReport {
    /// The logged reply's receipt.
    pub receipt: StoreReceipt,
    /// Work to run off the turn (a belief build, per the policy).
    pub jobs: Vec<BackgroundJob>,
}

/// Turns about to leave the host's prompt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Compaction {
    /// The conversation thread.
    pub thread_id: String,
    /// The turns being dropped, oldest first. They are already stored; they
    /// steer what is recalled.
    pub dropped: Vec<Turn>,
    /// What the conversation is about now, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focus: Option<String>,
}
