//! Domain types for the summary trees.
//!
//! Two shapes live here, and the file is ordered that way. The first is the
//! **markdown time tree**: summaries organised as a time hierarchy, root → year
//! → month → day → hour (leaf), ported from OpenHuman's
//! `memory_tree/tree_runtime/types.rs`. The second, below
//! [`node_id_to_path`], is the **sealed summary forest**: one tree per ingest
//! source, levelled by seal generation. They are navigated by different members
//! of the same family — see the section comment further down for why one cannot
//! answer for the other.

use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Hierarchical level of a tree node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NodeLevel {
    /// Single tree root; aggregates all years. Wire string `"root"`.
    Root,
    /// One node per calendar year. Wire string `"year"`.
    Year,
    /// One node per calendar month. Wire string `"month"`.
    Month,
    /// One node per calendar day. Wire string `"day"`.
    Day,
    /// Leaf level; one node per hour, where raw content lands. Wire string `"hour"`.
    Hour,
}

impl NodeLevel {
    /// Maximum number of tokens allowed at this level.
    pub fn max_tokens(&self) -> u32 {
        match self {
            Self::Hour => 1_000,
            Self::Day => 2_000,
            Self::Month => 4_000,
            Self::Year => 8_000,
            Self::Root => 20_000,
        }
    }

    /// The level above this one in the hierarchy (`None` for root).
    pub fn parent_level(&self) -> Option<NodeLevel> {
        match self {
            Self::Hour => Some(Self::Day),
            Self::Day => Some(Self::Month),
            Self::Month => Some(Self::Year),
            Self::Year => Some(Self::Root),
            Self::Root => None,
        }
    }

    /// True only for the leaf level (hour).
    pub fn is_leaf(&self) -> bool {
        matches!(self, Self::Hour)
    }

    /// Parse a level string from YAML frontmatter.
    pub fn from_str_label(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "root" => Some(Self::Root),
            "year" => Some(Self::Year),
            "month" => Some(Self::Month),
            "day" => Some(Self::Day),
            "hour" => Some(Self::Hour),
            _ => None,
        }
    }

    /// Label for display / frontmatter.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Root => "root",
            Self::Year => "year",
            Self::Month => "month",
            Self::Day => "day",
            Self::Hour => "hour",
        }
    }
}

/// A single node in the summary tree.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeNode {
    /// Path-style hierarchical id, e.g. `"2024/03/15/09"` or `"root"`.
    pub node_id: String,
    /// Namespace owning this tree (isolates independent trees).
    pub namespace: String,
    /// Hierarchical level this node sits at.
    pub level: NodeLevel,
    /// Id of the parent node; `None` only for the root.
    pub parent_id: Option<String>,
    /// Rolled-up summary text for this node.
    pub summary: String,
    /// Estimated token count of [`Self::summary`]; bounded by [`NodeLevel::max_tokens`].
    pub token_count: u32,
    /// Number of direct children rolled into this node.
    pub child_count: u32,
    /// Creation timestamp (UTC).
    pub created_at: DateTime<Utc>,
    /// Last-update timestamp (UTC).
    pub updated_at: DateTime<Utc>,
    /// Optional opaque metadata blob; omitted from serialization when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<String>,
}

/// Metadata about an entire tree within a namespace.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TreeStatus {
    /// Namespace the tree belongs to.
    pub namespace: String,
    /// Total number of nodes across all levels.
    pub total_nodes: u64,
    /// Number of populated levels (tree height).
    pub depth: u32,
    /// Timestamp of the earliest ingested entry, if any.
    pub oldest_entry: Option<DateTime<Utc>>,
    /// Timestamp of the most recent ingested entry, if any.
    pub newest_entry: Option<DateTime<Utc>>,
    /// When the tree was last (re)built or sealed.
    pub last_run_at: Option<DateTime<Utc>>,
}

/// Input for appending raw content to the ingestion buffer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestRequest {
    /// Target namespace to append content into.
    pub namespace: String,
    /// Raw content to buffer for summarization.
    pub content: String,
    /// Event time used to derive the hour leaf; defaults to ingestion time when absent.
    #[serde(default)]
    pub timestamp: Option<DateTime<Utc>>,
    /// Optional structured metadata carried alongside the content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

/// Result of a tree query at a specific node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    /// The node addressed by the query.
    pub node: TreeNode,
    /// Direct children of [`Self::node`], for drill-down navigation.
    pub children: Vec<TreeNode>,
}

/// Rough token estimate: ~4 characters per token.
pub fn estimate_tokens(text: &str) -> u32 {
    u32::try_from(text.len().div_ceil(4)).unwrap_or(u32::MAX)
}

/// Derive the parent node ID from a node ID.
pub fn derive_parent_id(node_id: &str) -> Option<String> {
    if node_id == "root" {
        return None;
    }
    match node_id.rfind('/') {
        Some(pos) => Some(node_id[..pos].to_string()),
        None => Some("root".to_string()),
    }
}

/// Determine the `NodeLevel` from a node ID string.
pub fn level_from_node_id(node_id: &str) -> NodeLevel {
    if node_id == "root" {
        return NodeLevel::Root;
    }
    match node_id.matches('/').count() {
        0 => NodeLevel::Year,
        1 => NodeLevel::Month,
        2 => NodeLevel::Day,
        _ => NodeLevel::Hour,
    }
}

/// Derive all ancestor node IDs from a timestamp (hour through root).
/// Returns `(hour_id, day_id, month_id, year_id, root_id)`.
pub fn derive_node_ids(ts: &DateTime<Utc>) -> (String, String, String, String, String) {
    let year = format!("{}", ts.year());
    let month = format!("{}/{:02}", ts.year(), ts.month());
    let day = format!("{}/{:02}/{:02}", ts.year(), ts.month(), ts.day());
    let hour = format!(
        "{}/{:02}/{:02}/{:02}",
        ts.year(),
        ts.month(),
        ts.day(),
        ts.hour()
    );
    (hour, day, month, year, "root".to_string())
}

/// Convert a node ID to a relative file path within the tree directory.
pub fn node_id_to_path(node_id: &str) -> PathBuf {
    if node_id == "root" {
        return PathBuf::from("root.md");
    }
    if node_id.starts_with('/')
        || node_id
            .split('/')
            .any(|part| part.is_empty() || !part.chars().all(|c| c.is_ascii_digit()))
    {
        return PathBuf::from("invalid");
    }
    let level = level_from_node_id(node_id);
    if level.is_leaf() {
        PathBuf::from(format!("{node_id}.md"))
    } else {
        PathBuf::from(node_id).join("summary.md")
    }
}

// ── The sealed summary forest ─────────────────────────────────────────────
//
// Everything above describes the *markdown time tree*: one node per hour, day,
// month and year, addressed as `2024/03/15/09`, one `.md` file each, navigated
// by `MemoryTree::drill_down`. The types below describe a second shape — the
// sealed summary **forest**: one tree per ingest source rather than one per
// calendar, levelled by seal generation rather than by calendar unit, and with
// no calendar-shaped node id for `drill_down` to address it by.
//
// The contract does not require a driver to keep the two apart. The embedded
// engine happens to — the markdown tree is files, the forest is tables — and a
// driver with a single structure answers both surfaces from it. What the
// contract does require is that both are reachable, because a host that can
// reach only the first has to read the second out of the driver's storage to
// draw it, which is the split-brain this contract exists to end.

/// Maximum length, in characters, of [`TreeLeaf::preview`].
///
/// Fixed here rather than left to each driver because the preview is a *label*
/// and the caller lays it out: a driver that returned whole bodies would blow
/// the frame budget on a forest-sized read, and one that returned forty
/// characters would silently truncate a caller that had budgeted for more. A
/// caller that wants the body asks
/// `MemoryChunks::chunk_detail` for the one leaf it is
/// showing, which is a single row rather than every row.
pub const LEAF_PREVIEW_CHARS: usize = 200;

/// One sealed summary node, with the tree it belongs to denormalised onto it.
///
/// # Why not a ranked hit
///
/// This overlaps `RetrievalHit` in almost every field and is deliberately not
/// it. A hit carries a `score`, which is meaningless for a structural walk —
/// nothing was ranked and there was no query to rank against — and it carries
/// no `parent_id`, because a ranked list is flat and has no reason to. The
/// parent link is the whole point here: it is the edge a caller draws a graph
/// from, and reconstructing it from each node's `child_ids` means holding the
/// entire forest in memory first, which is exactly what a truncated read
/// cannot do.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeSummary {
    /// Stable summary-node id, unique across the store.
    pub id: String,
    /// Id of the tree this node was sealed into.
    pub tree_id: String,
    /// The owning tree's kind — `source`, `topic`, `global`, ….
    ///
    /// Open vocabulary, and a plain string for the same reason
    /// `MemorySourceSink::accept_source_items` takes
    /// its `source_kind` as one: the set belongs to the driver and grows
    /// without a contract change. A caller that does not recognise a kind must
    /// still render the node.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tree_kind: String,
    /// The owning tree's scope — what it covers, e.g. `slack:#eng`,
    /// `github:acme/widget`. Empty when the driver does not scope its trees.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tree_scope: String,
    /// Seal generation: `1` for a summary over raw leaves, `2` over `1`, and so
    /// on. Never `0` — a leaf is a [`TreeLeaf`], not a summary at level zero.
    pub level: u32,
    /// Parent summary id, or `None` while this node is its tree's current root.
    ///
    /// A `Some` that names a node **absent from the same read** is expected
    /// rather than a fault: the parent may sit beyond the read's bound, or the
    /// scope may allow this node's tree and not its parent's. A caller
    /// building edges must treat an unresolvable parent as a root, which is
    /// what the host's Memory tab does.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// The children sealed under this node, fixed at seal time: leaf ids at
    /// level 1, lower-level summary ids above it.
    ///
    /// Not every level-1 child id resolves to a [`TreeLeaf`]. A document tree
    /// seals over logical units — a commit, an issue, a page — whose ids never
    /// existed as chunk rows, so a caller must label an unresolved child from
    /// the id itself rather than assuming a lookup will find it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub child_ids: Vec<String>,
    /// Inclusive start of the time span this node's children cover.
    pub time_range_start: DateTime<Utc>,
    /// Inclusive end of the time span this node's children cover.
    pub time_range_end: DateTime<Utc>,
    /// The node's text, cut to a label as [`TreeLeaf::preview`] is, when the
    /// driver serves it inline.
    ///
    /// `None` from a driver that keeps each summary as a file in its content
    /// vault, as the embedded engine does, where a caller reads the text by
    /// path. `Some` from a driver that keeps no such file — hosted memory,
    /// whose nodes are the server's concepts, beliefs and facts — and a caller
    /// must not look for a file for such a node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preview: Option<String>,
}

/// One leaf and the summary it was sealed under.
///
/// The back-pointer is why this is not `MemoryChunks::list_chunks`: a chunk row
/// says nothing about which summary claimed it, and that link is the edge
/// between the forest's bottom level and the content under it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreeLeaf {
    /// The leaf's chunk id — the same id `MemoryChunks` addresses it by.
    pub chunk_id: String,
    /// The summary that sealed over this leaf, or `None` when nothing has
    /// sealed it yet.
    ///
    /// `None` is the normal state of freshly-ingested content, not an error:
    /// sealing is a scheduled step the host drives, so an unsealed leaf is one
    /// the scheduler has not reached.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_summary_id: Option<String>,
    /// The logical source this leaf came from.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_id: String,
    /// A label for the leaf: its first non-empty line, truncated to
    /// [`LEAF_PREVIEW_CHARS`] characters.
    ///
    /// Characters, not bytes — a byte cut would split a multi-byte codepoint,
    /// and a driver that answered with invalid UTF-8 would fail to encode
    /// rather than return a short label.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub preview: String,
    /// Inclusive start of the leaf's time coverage.
    pub time_range_start: DateTime<Utc>,
    /// Inclusive end of the leaf's time coverage.
    pub time_range_end: DateTime<Utc>,
}

/// A bounded walk of the sealed summaries in a store.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryForest {
    /// The nodes, ordered by tree, then level, then seal time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub summaries: Vec<TreeSummary>,
    /// Whether the walk stopped at a bound rather than at the end of the store.
    ///
    /// A bound is not an error — the same reading `GraphView::truncated`
    /// takes — but it is not a uniform thinning either. The order is
    /// tree-major, so a truncated walk drops **whole trees** off the tail
    /// rather than sampling across them: a caller that renders this as the
    /// complete picture is showing a store with sources missing, and one that
    /// counts nodes from it is counting a prefix. Say so in the UI, or raise
    /// the bound and read again.
    ///
    /// Always serialized, unlike the fields above: a caller that has to notice
    /// this must not have it disappear from the payload when it is `false`,
    /// because "absent" and "not truncated" are then the same bytes and the
    /// only reading left is the optimistic one.
    #[serde(default)]
    pub truncated: bool,
}

/// First non-empty line of `content`, truncated to [`LEAF_PREVIEW_CHARS`].
///
/// Here rather than in each driver so two drivers cannot disagree about what a
/// preview is, and so the host is not left re-deriving it from a body the
/// forest read deliberately does not carry.
pub fn leaf_preview(content: &str) -> String {
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .chars()
        .take(LEAF_PREVIEW_CHARS)
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// The summariser door, and the roots it eventually produces.
// ─────────────────────────────────────────────────────────────────────────────
//
// Everything above describes a tree that has already been sealed. The three
// shapes below describe the *act* of sealing one level of it — N contributions
// from level `n` folded into the single node that lands at level `n + 1` — and
// the fourth describes the top of what that folding leaves behind.
//
// # Why the fold has to be a contract member
//
// It is the one step in the tree pipeline that is neither deterministic nor
// local. It costs an inference call, that call is billed, and the provider
// making it is configured on the *driver's* side of the boundary. A host that
// reaches memory only over the module has no chat provider of its own to fold
// with and no rate card to charge against, so the fold has to happen where the
// provider is — and the usage has to come back attached to the text, because
// nothing downstream can recompute it from the words.
//
// That is why [`SummaryOutput`] here carries seven fields rather than the four
// the engine's own deterministic summarisers return: `input_tokens`,
// `output_tokens` and `charged_amount_usd` exist only because a provider was
// paid, and a caller doing cost accounting cannot derive them.
//
// # Every budget crosses, and none of them is defaulted
//
// [`SummaryContext`] carries three separate token numbers and they are not
// interchangeable. `token_budget` clamps the *output*; `input_token_budget` is
// the whole context the fold may occupy; `overhead_reserve_tokens` is the
// prompt and formatting headroom withheld from the sources. The driver divides
// what is left after the other two among the inputs, so a caller that omits
// one and lets it default to zero does not get a slightly different summary —
// it gets every source clamped to nothing and a fold with no evidence in it.
// They are required fields for that reason, not optional ones with a sensible
// fallback: there is no sensible fallback.
//
// [`SummaryContext::ask`] is load-bearing in the same way and in the opposite
// direction: its presence selects an entirely different system prompt. A
// flavoured tree folded without its ask produces a generic digest where the
// caller expected a profile, and nothing in the output says which prompt ran.
//
// # `tree_kind` crosses as a string, not as an enum
//
// The engine's `TreeKind` is `#[non_exhaustive]` and has already grown a fourth
// variant (`flavoured`). A closed enum on the wire would mean the first payload
// naming a kind this build predates fails to *deserialize*, taking the whole
// frame with it — an unfamiliar label degrading into a hard decode failure.
// [`crate::provider::retrieval`] documents that argument at length for
// `RetrievalHit::tree_kind` and `EntityMatch::kind`; this is the same rule
// applied to the same enum, and the hazard is symmetric here because this field
// travels in a *request*: a newer caller naming a newer kind must not make an
// older driver fail to parse the call.
//
// Known values today are `source`, `topic`, `global` and `flavoured`. Unlike
// the response-side fields, though, this one is validated on arrival — a driver
// that cannot map the string onto a kind it understands answers
// [`MemoryError::Invalid`](crate::error::MemoryError::Invalid) naming it,
// because folding under the wrong kind silently mislabels a summary and nothing
// afterwards can tell that it happened.

/// One contribution being folded — a raw leaf at level 0 on its way to level 1,
/// or a lower-level summary on its way to the level above it.
///
/// Owned throughout, where the engine's own twin borrows: this shape is
/// serialized into a frame, and a borrow cannot outlive the call that decoded
/// it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SummaryInput {
    /// Machine-readable id of the contributing leaf or lower-level summary.
    ///
    /// It is written into the prompt as the provenance marker for its block, so
    /// it is not decorative: two inputs sharing an id produce a fold whose
    /// sources cannot be told apart afterwards.
    pub id: String,
    /// Raw text being folded into the parent summary.
    ///
    /// Clamped driver-side to this input's share of the context budget; an
    /// input whose content is blank after trimming is dropped from the prompt
    /// entirely rather than contributing an empty block.
    pub content: String,
    /// Approximate token count of [`content`](Self::content).
    pub token_count: u32,
    /// Canonical entity ids attached to this input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<String>,
    /// Topic labels attached to this input.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topics: Vec<String>,
    /// Start of the time window this input covers (inclusive).
    pub time_range_start: DateTime<Utc>,
    /// End of the time window this input covers (inclusive).
    pub time_range_end: DateTime<Utc>,
    /// Importance weight; higher-scoring inputs are folded first and are least
    /// likely to be dropped under budget pressure.
    ///
    /// The ordering is the driver's, and it is applied to the whole slice
    /// before any clamping, so the order the caller sends inputs in does not
    /// change the result — the scores do.
    pub score: f32,
}

/// Per-seal context: which tree and level is being sealed, and under what
/// budgets.
///
/// The engine's twin borrows its strings from the tree it was built over; this
/// one owns them, for the reason [`SummaryInput`] gives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryContext {
    /// Machine-readable id of the tree being sealed.
    pub tree_id: String,
    /// Wire kind of the tree: `source`, `topic`, `global`, `flavoured`, ….
    ///
    /// A string rather than an enum, and validated by the driver rather than by
    /// serde — see the section note above this type for both halves of that
    /// decision.
    pub tree_kind: String,
    /// Level the produced summary lands at; inputs come from `target_level - 1`.
    pub target_level: u32,
    /// Maximum approximate tokens the produced summary may occupy.
    ///
    /// Both an instruction and a clamp: it is stated in the prompt *and*
    /// enforced on the returned text, so a provider that overruns it is
    /// truncated rather than trusted.
    pub token_budget: u32,
    /// Total input/context budget available to this fold.
    ///
    /// What is left of it after [`token_budget`](Self::token_budget) and
    /// [`overhead_reserve_tokens`](Self::overhead_reserve_tokens) are withheld
    /// is divided evenly among the inputs. A value smaller than those two
    /// leaves every input a share of zero, which is a fold over nothing.
    pub input_token_budget: u32,
    /// Prompt and formatting headroom withheld from source inputs.
    pub overhead_reserve_tokens: u32,
    /// Natural-language ask that steers the fold, for flavoured trees.
    ///
    /// `Some` selects a flavour-directed system prompt that distils the inputs
    /// into a running profile answering the ask; `None` selects the generic
    /// folding prompt every other tree kind uses. An ask that is present but
    /// blank reads as `None`.
    ///
    /// This is not a hint the driver may drop. Sending `None` for a tree that
    /// has an ask produces a well-formed summary of the wrong kind, and the
    /// response carries no field that says which prompt ran.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ask: Option<String>,
}

/// The folded summary, with what the provider charged to produce it.
///
/// # Why this shape and not the engine's four-field one
///
/// The engine has two summary outputs. The deterministic in-crate summarisers
/// return content, tokens, entities and topics; the provider-backed fold
/// returns those plus the usage the call incurred. This is the second one,
/// deliberately: a fold that crosses the module boundary is always the billed
/// one — the deterministic path never leaves the driver — so dropping the usage
/// fields here would lose the only copy of a number the host meters spend
/// against.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SummaryOutput {
    /// Folded summary text, clamped to the seal's token budget.
    ///
    /// Empty when there was nothing to fold — every input blank, or the slice
    /// itself empty. That is a successful no-op rather than an error: a seal
    /// over an empty buffer is idempotent for the same reason
    /// `MemoryTree::seal` is.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub content: String,
    /// Approximate token count of [`content`](Self::content).
    #[serde(default)]
    pub token_count: u32,
    /// Canonical entity ids for the summary.
    ///
    /// Emitted empty by the provider fold: entity labelling happens separately,
    /// at seal time, under the tree's own label strategy. Carried anyway so a
    /// driver whose summariser does extract them has somewhere to put them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<String>,
    /// Topic labels for the summary; empty on the same terms as
    /// [`entities`](Self::entities).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub topics: Vec<String>,
    /// Prompt tokens the provider reported for this fold, or `0` when it
    /// reported no usage at all.
    ///
    /// Zero is "not reported", not "free". Providers differ in whether they
    /// return usage, and a caller that treats zero as a measured floor
    /// under-counts spend rather than over-counting it.
    #[serde(default)]
    pub input_tokens: u64,
    /// Completion tokens the provider reported, on the same terms as
    /// [`input_tokens`](Self::input_tokens).
    #[serde(default)]
    pub output_tokens: u64,
    /// What the provider said the call cost, in USD.
    ///
    /// `None` when the provider quoted nothing, and also when it quoted zero —
    /// a zero charge is indistinguishable from an unpriced one, so it is
    /// reported as absent rather than as a free call the caller would then add
    /// to a running total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub charged_amount_usd: Option<f64>,
}

/// One namespace's root summary, as a bounded read returns it.
///
/// A named shape rather than the engine's `(String, String, DateTime)` tuple.
/// The tuple is unambiguous at the one call site that builds it and is nothing
/// but positional on the wire, where the two `String`s are the same type and
/// swapping them produces a payload that decodes cleanly and means the
/// opposite. Naming the fields is what makes that a compile error instead of a
/// mislabelled memory tab.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RootSummary {
    /// The namespace the summary is the root of.
    pub namespace: String,
    /// The root summary text, already truncated to the caller's caps.
    ///
    /// A body that was cut carries a trailing `[... truncated]` marker, so a
    /// caller can tell a clipped summary from a short one without comparing
    /// lengths against the caps it asked for.
    pub body: String,
    /// When the root node was last written.
    pub updated_at: DateTime<Utc>,
}

#[cfg(test)]
#[path = "tree_tests.rs"]
mod tests;
