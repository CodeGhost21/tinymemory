//! A deliberately tiny scripted agent.
//!
//! It has no model. Each user turn runs the real lifecycle calls, so
//! everything the memory layer does is the same as for a real agent:
//!
//! 1. `pre_turn` logs the user's text and recalls a context pack, leaving
//!    out the turns still in its prompt window.
//! 2. The agent "runs" the scripted tool calls and copies their results into
//!    its reply. Memory keeps only a call's name and id, so a result the
//!    reply leaves out is lost.
//! 3. It answers a question with the pack line that shares the most words
//!    with it ([`answer`]), and otherwise acknowledges.
//! 4. `post_turn` logs the reply with its tool calls.
//!
//! The extractive answer is a stand-in for a model reading the pack. It
//! scores what a model would see, not how well some model reasons.

use std::time::Instant;

use chrono::{DateTime, Duration, Utc};
use tinymemory_api::ToolCallRef;
use tinymemory_tools::{AgentMemory, BackgroundJob, ContextPack, PostTurn, PreTurn};

/// A scripted tool call and the result the "tool" returns.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ToolStep {
    /// The tool's name.
    pub(crate) name: &'static str,
    /// What it returned.
    pub(crate) result: &'static str,
}

/// What one scripted turn did and how long each step took.
#[derive(Debug, Clone)]
pub(crate) struct TurnRecord {
    /// `pre_turn` latency, in milliseconds.
    pub(crate) pre_ms: f64,
    /// `post_turn` latency, in milliseconds.
    pub(crate) post_ms: f64,
    /// Whether the user turn was logged.
    pub(crate) logged: bool,
    /// The jobs `post_turn` handed back.
    pub(crate) jobs: Vec<BackgroundJob>,
    /// How many tool calls the reply made.
    pub(crate) tool_calls: usize,
}

/// One conversation thread driven by the script.
pub(crate) struct ScriptedAgent {
    memory: AgentMemory,
    thread: String,
    next: u32,
    /// How many of the thread's turns stay in the prompt.
    window: u32,
    clock: Option<DateTime<Utc>>,
}

impl ScriptedAgent {
    /// A new thread for `memory`, keeping the last `window` turns in the
    /// prompt.
    pub(crate) fn new(memory: AgentMemory, thread: impl Into<String>, window: u32) -> Self {
        Self {
            memory,
            thread: thread.into(),
            next: 0,
            window,
            clock: None,
        }
    }

    /// Timestamps the thread's turns from `at`, a minute apart.
    pub(crate) fn at(mut self, at: DateTime<Utc>) -> Self {
        self.clock = Some(at);
        self
    }

    /// The first turn index still in the prompt.
    fn in_prompt_from(&self) -> u32 {
        self.next.saturating_sub(self.window)
    }

    /// The timestamp of the next turn, if the thread is timed.
    fn tick(&mut self) -> Option<DateTime<Utc>> {
        let at = self.clock?;
        self.clock = Some(at + Duration::minutes(1));
        Some(at)
    }

    /// One exchange: the user says `text`, the agent calls `tools` and
    /// replies.
    pub(crate) async fn user(
        &mut self,
        text: &str,
        tools: &[ToolStep],
    ) -> Result<TurnRecord, tinymemory_api::Error> {
        let mut pre = PreTurn::new(&self.thread, self.next, text);
        pre.in_prompt_from = self.in_prompt_from();
        pre.at = self.tick();
        let started = Instant::now();
        let context = self.memory.pre_turn(pre).await?;
        let pre_ms = ms(started);

        let reply = reply(&context.pack, text, tools);
        let mut post = PostTurn::new(&self.thread, self.next + 1, reply);
        post.tool_calls = tools
            .iter()
            .enumerate()
            .map(|(index, step)| ToolCallRef {
                name: step.name.to_string(),
                id: Some(format!("{}-{}-{index}", self.thread, self.next)),
            })
            .collect();
        post.at = self.tick();
        let started = Instant::now();
        let report = self.memory.post_turn(post).await?;
        let post_ms = ms(started);
        self.next += 2;
        Ok(TurnRecord {
            pre_ms,
            post_ms,
            logged: context.logged.is_some(),
            jobs: report.jobs,
            tool_calls: tools.len(),
        })
    }
}

/// The agent's reply: tool results first, then an answer or an
/// acknowledgement.
fn reply(pack: &ContextPack, text: &str, tools: &[ToolStep]) -> String {
    let mut lines: Vec<String> = tools
        .iter()
        .map(|step| format!("{} returned: {}", step.name, step.result))
        .collect();
    if text.trim_end().ends_with('?') {
        lines.push(match answer(&pack.markdown, text) {
            Some(line) => format!("Going by memory: {line}"),
            None => "I don't have that in memory.".to_string(),
        });
    } else if lines.is_empty() {
        lines.push("Noted.".to_string());
    }
    lines.join("\n")
}

/// Words too common to tell two lines apart.
const STOPWORDS: [&str; 42] = [
    "a", "an", "the", "is", "are", "was", "were", "do", "does", "did", "of", "to", "in", "on",
    "at", "for", "and", "or", "our", "we", "i", "my", "me", "you", "your", "what", "which", "who",
    "when", "where", "how", "it", "that", "this", "with", "be", "should", "can", "get", "have",
    "has", "from",
];

/// The lowercase content words of `text`.
fn words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric() && c != '-')
        .map(str::to_lowercase)
        .filter(|word| word.len() > 1 && !STOPWORDS.contains(&word.as_str()))
        .collect()
}

/// The pack line the agent would answer `question` with: the bullet sharing
/// the most content words with it (earlier sections win ties), skipping the
/// agent's own non-answers and lines that only repeat a question.
pub(crate) fn answer(markdown: &str, question: &str) -> Option<String> {
    let wanted = words(question);
    let mut best: Option<(usize, &str)> = None;
    for line in markdown.lines().filter_map(|line| line.strip_prefix("- ")) {
        let body = line.trim();
        if body.ends_with('?') || body.contains("I don't have that in memory") {
            continue;
        }
        let held = words(body);
        let overlap = wanted.iter().filter(|word| held.contains(word)).count();
        if overlap > 0 && best.is_none_or(|(score, _)| overlap > score) {
            best = Some((overlap, body));
        }
    }
    best.map(|(_, line)| line.to_string())
}

/// Milliseconds since `started`.
pub(crate) fn ms(started: Instant) -> f64 {
    started.elapsed().as_secs_f64() * 1e3
}
