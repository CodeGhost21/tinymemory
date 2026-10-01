//! Tests for the markdown time-tree node types, and for the summariser door's
//! wire shapes.

// A failed assertion in a test is a panic either way; `unwrap`/`expect` here say
// what the invariant was. Same allowance the crate's other test modules take.
#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use super::*;
use chrono::TimeZone;
use std::path::PathBuf;

#[test]
fn node_level_max_tokens() {
    assert_eq!(NodeLevel::Hour.max_tokens(), 1_000);
    assert_eq!(NodeLevel::Day.max_tokens(), 2_000);
    assert_eq!(NodeLevel::Month.max_tokens(), 4_000);
    assert_eq!(NodeLevel::Year.max_tokens(), 8_000);
    assert_eq!(NodeLevel::Root.max_tokens(), 20_000);
}

#[test]
fn node_level_parent_chain() {
    assert_eq!(NodeLevel::Hour.parent_level(), Some(NodeLevel::Day));
    assert_eq!(NodeLevel::Day.parent_level(), Some(NodeLevel::Month));
    assert_eq!(NodeLevel::Month.parent_level(), Some(NodeLevel::Year));
    assert_eq!(NodeLevel::Year.parent_level(), Some(NodeLevel::Root));
    assert_eq!(NodeLevel::Root.parent_level(), None);
}

#[test]
fn derive_parent_id_chain() {
    assert_eq!(derive_parent_id("2024/03/15/14"), Some("2024/03/15".into()));
    assert_eq!(derive_parent_id("2024/03/15"), Some("2024/03".into()));
    assert_eq!(derive_parent_id("2024/03"), Some("2024".into()));
    assert_eq!(derive_parent_id("2024"), Some("root".into()));
    assert_eq!(derive_parent_id("root"), None);
}

#[test]
fn level_from_node_id_all_levels() {
    assert_eq!(level_from_node_id("root"), NodeLevel::Root);
    assert_eq!(level_from_node_id("2024"), NodeLevel::Year);
    assert_eq!(level_from_node_id("2024/03"), NodeLevel::Month);
    assert_eq!(level_from_node_id("2024/03/15"), NodeLevel::Day);
    assert_eq!(level_from_node_id("2024/03/15/14"), NodeLevel::Hour);
}

#[test]
fn derive_node_ids_from_timestamp() {
    let ts = Utc.with_ymd_and_hms(2024, 3, 15, 14, 30, 0).unwrap();
    let (hour, day, month, year, root) = derive_node_ids(&ts);
    assert_eq!(hour, "2024/03/15/14");
    assert_eq!(day, "2024/03/15");
    assert_eq!(month, "2024/03");
    assert_eq!(year, "2024");
    assert_eq!(root, "root");
}

#[test]
fn node_id_to_path_mapping() {
    assert_eq!(node_id_to_path("root"), PathBuf::from("root.md"));
    assert_eq!(node_id_to_path("2024"), PathBuf::from("2024/summary.md"));
    assert_eq!(
        node_id_to_path("2024/03"),
        PathBuf::from("2024/03/summary.md")
    );
    assert_eq!(
        node_id_to_path("2024/03/15/14"),
        PathBuf::from("2024/03/15/14.md")
    );
}

#[test]
fn estimate_tokens_rough() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("abcd"), 1);
    assert_eq!(estimate_tokens(&"a".repeat(4000)), 1000);
}

#[test]
fn node_level_roundtrip() {
    for level in [
        NodeLevel::Root,
        NodeLevel::Year,
        NodeLevel::Month,
        NodeLevel::Day,
        NodeLevel::Hour,
    ] {
        assert_eq!(NodeLevel::from_str_label(level.as_str()), Some(level));
    }
}

#[test]
fn a_summary_context_decodes_a_tree_kind_this_build_has_never_heard_of() {
    // The reason `tree_kind` is a `String`. `TreeKind` is `#[non_exhaustive]`
    // and has already grown a fourth variant, so a closed enum here would turn
    // the first payload naming a fifth into a *decode* failure that takes the
    // whole frame with it — a summarise call that fails outright rather than a
    // label nothing recognises. Asserted with an invented kind rather than with
    // `flavoured`, because `flavoured` would pass even against a closed enum
    // that already knows it.
    let raw = serde_json::json!({
        "tree_id": "tree-1",
        "tree_kind": "a-kind-invented-after-this-build",
        "target_level": 2,
        "token_budget": 800,
        "input_token_budget": 6_000,
        "overhead_reserve_tokens": 400,
    });
    let context: SummaryContext =
        serde_json::from_value(raw).expect("an unknown kind still parses");
    assert_eq!(context.tree_kind, "a-kind-invented-after-this-build");
    assert_eq!(context.ask, None, "an absent ask is the generic fold");
}

#[test]
fn a_summary_context_round_trips_all_three_budgets_and_its_ask() {
    // Each of these is load-bearing on its own: `token_budget` clamps the
    // output, `input_token_budget` is the whole context, and
    // `overhead_reserve_tokens` is withheld from the sources before the rest is
    // divided. A field that silently failed to cross would not error — it would
    // default to zero and produce a fold over nothing, which reads downstream
    // as a model that returned little.
    let context = SummaryContext {
        tree_id: "tree-7".to_string(),
        tree_kind: "flavoured".to_string(),
        target_level: 3,
        token_budget: 900,
        input_token_budget: 8_000,
        overhead_reserve_tokens: 512,
        ask: Some("how does this person write".to_string()),
    };
    let encoded = serde_json::to_string(&context).unwrap();
    let decoded: SummaryContext = serde_json::from_str(&encoded).unwrap();
    assert_eq!(decoded, context);

    // The ask changes which system prompt runs, so its absence has to be
    // distinguishable from its presence rather than encoded as an empty string.
    let generic = SummaryContext {
        ask: None,
        ..context
    };
    let payload: serde_json::Value = serde_json::to_value(&generic).unwrap();
    assert!(
        payload.get("ask").is_none(),
        "an absent ask is absent from the payload, not an empty one"
    );
}

#[test]
fn a_summary_input_keeps_its_score_and_its_window() {
    let start = Utc.with_ymd_and_hms(2024, 3, 15, 10, 0, 0).unwrap();
    let end = Utc.with_ymd_and_hms(2024, 3, 15, 11, 0, 0).unwrap();
    let input = SummaryInput {
        id: "chunk-1".to_string(),
        content: "the body being folded".to_string(),
        token_count: 5,
        entities: vec!["person:ada".to_string()],
        topics: vec!["release".to_string()],
        time_range_start: start,
        time_range_end: end,
        score: 0.75,
    };
    let decoded: SummaryInput =
        serde_json::from_str(&serde_json::to_string(&input).unwrap()).unwrap();
    assert_eq!(decoded, input);
    // The score orders the fold and decides what survives budget pressure, so
    // it must cross as the float it is rather than being rounded on the way.
    assert!((decoded.score - 0.75).abs() < f32::EPSILON);
}

#[test]
fn a_summary_output_reports_no_usage_as_zero_and_no_charge_as_absent() {
    // The default is what a fold with nothing to fold returns, and it has to
    // decode from a payload that omits every optional field — an older peer's
    // shape, and also the cheapest thing a driver can send.
    let empty: SummaryOutput = serde_json::from_str("{}").unwrap();
    assert_eq!(empty, SummaryOutput::default());
    assert!(empty.content.is_empty());
    assert_eq!(empty.input_tokens, 0);
    assert_eq!(
        empty.charged_amount_usd, None,
        "an unpriced call is absent, not a zero a caller would add to a total"
    );

    let billed = SummaryOutput {
        content: "folded".to_string(),
        token_count: 2,
        entities: Vec::new(),
        topics: Vec::new(),
        input_tokens: 1_200,
        output_tokens: 300,
        charged_amount_usd: Some(0.0042),
    };
    let decoded: SummaryOutput =
        serde_json::from_str(&serde_json::to_string(&billed).unwrap()).unwrap();
    assert_eq!(decoded, billed);
}

#[test]
fn a_root_summary_travels_by_name_so_its_two_strings_cannot_be_swapped() {
    // The whole reason this is not the engine's `(String, String, DateTime)`
    // tuple. Positionally, `namespace` and `body` are the same type: a producer
    // that emitted them the other way round would encode cleanly, decode
    // cleanly, and put a whole summary where a namespace label belongs.
    let summary = RootSummary {
        namespace: "team".to_string(),
        body: "what the team did\n\n[... truncated]".to_string(),
        updated_at: Utc.with_ymd_and_hms(2024, 3, 15, 14, 0, 0).unwrap(),
    };
    let payload = serde_json::to_value(&summary).unwrap();
    assert_eq!(payload["namespace"], "team");
    assert_eq!(payload["body"], "what the team did\n\n[... truncated]");
    let decoded: RootSummary = serde_json::from_value(payload).unwrap();
    assert_eq!(decoded, summary);
}

#[test]
fn a_tree_node_round_trips_with_its_level_spelled_as_the_files_spell_it() {
    // `TreeNode` predates the runtime-tree members but never crossed a frame
    // as a *response* until `RuntimeReadNode`/`RuntimeReadChildren`/
    // `RuntimeSummarize` — this pins the shape those members now serve. The
    // level's wire string matters doubly: it is also the spelling the engine's
    // markdown frontmatter uses, so a rename here would not just break decode,
    // it would disagree with every node already on disk.
    let node = TreeNode {
        node_id: "2024/03/15/09".to_string(),
        namespace: "team".to_string(),
        level: NodeLevel::Hour,
        parent_id: Some("2024/03/15".to_string()),
        summary: "the morning standup, folded".to_string(),
        token_count: 7,
        child_count: 0,
        created_at: Utc.with_ymd_and_hms(2024, 3, 15, 9, 0, 0).unwrap(),
        updated_at: Utc.with_ymd_and_hms(2024, 3, 15, 9, 30, 0).unwrap(),
        metadata: None,
    };
    let payload = serde_json::to_value(&node).unwrap();
    assert_eq!(payload["level"], "hour");
    assert_eq!(payload["node_id"], "2024/03/15/09");
    assert!(
        payload.get("metadata").is_none(),
        "absent metadata is omitted, not serialized as null"
    );
    let decoded: TreeNode = serde_json::from_value(payload).unwrap();
    assert_eq!(decoded.node_id, node.node_id);
    assert_eq!(decoded.level, node.level);
    assert_eq!(decoded.parent_id, node.parent_id);
    assert_eq!(decoded.summary, node.summary);
    assert_eq!(decoded.updated_at, node.updated_at);
    assert_eq!(decoded.metadata, None);
}

#[test]
fn a_tree_status_keeps_its_absent_timestamps_absent() {
    // The status of a namespace that has never been sealed is all-`None`, and
    // `RuntimeTreeStatus` serves exactly that on a fresh workspace. The three
    // options must decode back to `None` rather than to an epoch, because a
    // dashboard renders `oldest_entry` as coverage and an epoch reads as
    // "since 1970".
    let empty = TreeStatus {
        namespace: "team".to_string(),
        total_nodes: 0,
        depth: 0,
        oldest_entry: None,
        newest_entry: None,
        last_run_at: None,
    };
    let decoded: TreeStatus =
        serde_json::from_str(&serde_json::to_string(&empty).unwrap()).unwrap();
    assert_eq!(decoded.namespace, "team");
    assert_eq!(decoded.total_nodes, 0);
    assert_eq!(decoded.oldest_entry, None);
    assert_eq!(decoded.last_run_at, None);

    let run_at = Utc.with_ymd_and_hms(2024, 3, 15, 10, 0, 0).unwrap();
    let populated = TreeStatus {
        namespace: "team".to_string(),
        total_nodes: 12,
        depth: 5,
        oldest_entry: Some(Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap()),
        newest_entry: Some(run_at),
        last_run_at: Some(run_at),
    };
    let decoded: TreeStatus =
        serde_json::from_str(&serde_json::to_string(&populated).unwrap()).unwrap();
    assert_eq!(decoded.total_nodes, 12);
    assert_eq!(decoded.depth, 5);
    assert_eq!(decoded.newest_entry, Some(run_at));
}

#[test]
fn a_summary_without_a_preview_sends_nothing_and_an_older_one_decodes() {
    // A driver that keeps each summary as a file leaves `preview` out, so its
    // wire bytes are what they were before the field existed; and a summary
    // an older driver sent, with no `preview` key, decodes to `None`.
    let at = Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0).unwrap();
    let summary = TreeSummary {
        id: "s1".to_string(),
        tree_id: "t1".to_string(),
        tree_kind: "understanding".to_string(),
        tree_scope: "global".to_string(),
        level: 2,
        parent_id: None,
        child_ids: vec!["f1".to_string()],
        time_range_start: at,
        time_range_end: at,
        preview: None,
    };
    let wire = serde_json::to_value(&summary).unwrap();
    assert!(wire.get("preview").is_none(), "{wire}");
    let decoded: TreeSummary = serde_json::from_value(wire).unwrap();
    assert_eq!(decoded, summary);

    let served = TreeSummary {
        preview: Some("Prefers terse answers".to_string()),
        ..summary
    };
    let decoded: TreeSummary =
        serde_json::from_str(&serde_json::to_string(&served).unwrap()).unwrap();
    assert_eq!(decoded.preview.as_deref(), Some("Prefers terse answers"));
}
