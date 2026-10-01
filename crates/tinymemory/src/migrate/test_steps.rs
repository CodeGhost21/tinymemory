//! `migrate::copy_all`: each step past the keyed records, between fakes that
//! serve the families the step needs.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::panic)]

use std::collections::HashMap;

use crate::api::chrono::{TimeZone, Utc};
use crate::capabilities::Capability;
use crate::chunks::{Chunk, DataSource, Metadata, SourceKind};
use crate::goals::{GoalItem, GoalsDoc};
use crate::provider::{
    ConversationSegment, EpisodicEvent, EpisodicTurn, EventKind, FacetState, FacetType,
    ProfileFacet, SegmentEmbedding, SegmentStatus, UserState,
};
use crate::types::{MemoryTaint, StoredMemoryDocument};

use super::content::{data_source, taint_of};
use super::episodic::remap_turn_ids;
use super::test_support::{Fake, FRESH_IDS};
use super::{copy_all, CopyOptions, MigrateStep};

const EVERYTHING: [Capability; 6] = [
    Capability::Documents,
    Capability::Goals,
    Capability::Profile,
    Capability::EpisodicPortability,
    Capability::Chunks,
    Capability::Ingest,
];

fn document(namespace: &str, key: &str, title: &str, tags: &[&str]) -> StoredMemoryDocument {
    StoredMemoryDocument {
        document_id: format!("doc-{key}"),
        namespace: namespace.into(),
        key: key.into(),
        title: title.into(),
        content: format!("body of {key}"),
        source_type: "chat".into(),
        priority: "medium".into(),
        tags: tags.iter().map(|t| (*t).to_string()).collect(),
        metadata: serde_json::json!({}),
        category: "core".into(),
        session_id: None,
        created_at: 1.0,
        updated_at: 1.0,
        markdown_rel_path: String::new(),
        taint: MemoryTaint::Internal,
    }
}

fn facet(key: &str, value: &str, last_seen_at: f64) -> ProfileFacet {
    ProfileFacet {
        facet_id: format!("f-{key}"),
        facet_type: FacetType::Preference,
        key: key.into(),
        value: value.into(),
        confidence: 0.9,
        evidence_count: 2,
        source_segment_ids: None,
        first_seen_at: 1.0,
        last_seen_at,
        state: FacetState::default(),
        stability: 0.5,
        user_state: UserState::default(),
        evidence_refs: Vec::new(),
        class: None,
        cue_families: None,
    }
}

fn turn(id: i64, session: &str, content: &str, timestamp: f64) -> EpisodicTurn {
    EpisodicTurn {
        id: Some(id),
        session_id: session.into(),
        timestamp,
        role: "user".into(),
        content: content.into(),
        lesson: None,
        tool_calls_json: None,
        cost_microdollars: 0,
    }
}

fn chunk(kind: SourceKind, source_id: &str, seq: u32, minute: u32) -> (Chunk, String) {
    let at = Utc.with_ymd_and_hms(2026, 9, 1, 10, minute, 0).unwrap();
    let mut metadata = Metadata::point_in_time(kind, source_id, "owner", at);
    metadata.tags = vec![format!("tag-{seq}")];
    (
        Chunk {
            id: format!("{source_id}#{seq}"),
            content: "preview".into(),
            metadata,
            token_count: 3,
            seq_in_source: seq,
            created_at: at,
            partial_message: false,
        },
        format!("{source_id} part {seq}"),
    )
}

/// A source holding something in every family a step copies.
fn seeded_source() -> Fake {
    let source = Fake::serving(&EVERYTHING);
    {
        let mut documents = source.documents.lock().unwrap();
        for d in [
            document("notes", "plain", "plain", &[]),
            document("notes", "titled", "Trip plan", &["travel"]),
            document("source:drive-1", "synced", "A synced file", &[]),
        ] {
            documents.insert((d.namespace.clone(), d.key.clone()), d);
        }
    }
    *source.goals.lock().unwrap() = GoalsDoc {
        items: vec![
            GoalItem::new("g1", "ship the app"),
            GoalItem::new("g2", "learn rust"),
        ],
    };
    {
        let mut facets = source.facets.lock().unwrap();
        facets.insert("style/tone".into(), facet("style/tone", "terse", 50.0));
        facets.insert("style/length".into(), facet("style/length", "short", 10.0));
    }
    {
        let mut turns = source.turns.lock().unwrap();
        for t in [
            turn(1, "s1", "plan the trip", 1.0),
            turn(2, "s1", "book flights", 2.0),
        ] {
            turns.insert(t.id.unwrap(), t);
        }
    }
    source.segments.lock().unwrap().insert(
        "seg-1".into(),
        ConversationSegment {
            segment_id: "seg-1".into(),
            session_id: "s1".into(),
            namespace: "global".into(),
            start_episodic_id: 1,
            end_episodic_id: Some(2),
            start_timestamp: 1.0,
            end_timestamp: Some(2.0),
            turn_count: 2,
            summary: Some("planning a trip".into()),
            embedding: None,
            open: false,
            status: Some(SegmentStatus::Summarised),
            start_seq: None,
            end_seq: None,
        },
    );
    source.events.lock().unwrap().insert(
        "ev-1".into(),
        EpisodicEvent {
            event_id: "ev-1".into(),
            segment_id: "seg-1".into(),
            session_id: "s1".into(),
            namespace: "global".into(),
            kind: EventKind::Decision,
            content: "flights get booked".into(),
            subject: None,
            timestamp_ref: None,
            confidence: 0.9,
            embedding: None,
            source_turn_ids: Some("[1,2]".into()),
            created_at: 3.0,
        },
    );
    source.embeddings.lock().unwrap().insert(
        ("seg-1".into(), "sig".into()),
        SegmentEmbedding {
            segment_id: "seg-1".into(),
            model_signature: "sig".into(),
            embedding: vec![0.5],
            created_at: 3.0,
        },
    );
    {
        let mut chunks = source.chunks.lock().unwrap();
        // Listed newest first; the replay puts them back in sequence.
        chunks.push(chunk(SourceKind::Document, "notion:page-1", 1, 5));
        chunks.push(chunk(SourceKind::Document, "notion:page-1", 0, 9));
        chunks.push(chunk(SourceKind::Email, "gmail:thread-1", 0, 1));
        chunks.push(chunk(SourceKind::Chat, "conversations:agent", 0, 2));
        chunks.push(chunk(SourceKind::Document, "mem_src:folder-1:a.md", 0, 3));
    }
    source
}

/// A target that already holds a turn of its own under id 1, a goal, and a
/// newer reading of one facet.
fn seeded_target() -> Fake {
    let target = Fake::serving(&EVERYTHING);
    target
        .turns
        .lock()
        .unwrap()
        .insert(1, turn(1, "s0", "an older conversation", 0.5));
    *target.goals.lock().unwrap() = GoalsDoc {
        items: vec![GoalItem::new("g1", "Ship the app")],
    };
    target
        .facets
        .lock()
        .unwrap()
        .insert("style/tone".into(), facet("style/tone", "formal", 90.0));
    target
}

fn options() -> CopyOptions {
    CopyOptions {
        skip_source_prefixes: vec!["mem_src:".into()],
        ..CopyOptions::default()
    }
}

/// The document rejoined in sequence, mail and chat by message, and the
/// folder source left to the host's own sync.
fn assert_content_replayed(target: &Fake) {
    let ingested = target.ingested.lock().unwrap().clone();
    let by_method: HashMap<&str, Vec<_>> =
        ingested
            .iter()
            .fold(HashMap::new(), |mut acc, (method, items)| {
                acc.entry(*method)
                    .or_insert_with(Vec::new)
                    .extend(items.clone());
                acc
            });
    let notion = &by_method["document"];
    assert_eq!(notion.len(), 1);
    assert_eq!(
        notion[0].content,
        "notion:page-1 part 0\n\nnotion:page-1 part 1"
    );
    assert_eq!(notion[0].source, DataSource::Notion);
    assert_eq!(
        notion[0].tags,
        vec!["tag-0".to_string(), "tag-1".to_string()]
    );
    assert_eq!(notion[0].taint, MemoryTaint::ExternalSync);
    assert_eq!(by_method["email"][0].source, DataSource::Gmail);
    assert_eq!(by_method["chat"][0].taint, MemoryTaint::Internal);
    assert!(ingested
        .iter()
        .flat_map(|(_, items)| items)
        .all(|item| !item.source_id.starts_with("mem_src:")));
}

#[tokio::test]
async fn every_step_moves_what_the_target_lacks_and_a_rerun_writes_nothing() {
    let source = seeded_source();
    let target = seeded_target();
    let mut steps_seen = Vec::new();
    let report = copy_all(&source, &target, &options(), |p| {
        if steps_seen.last() != Some(&p.step) {
            steps_seen.push(p.step);
        }
    })
    .await
    .expect("copy");
    assert_eq!(report.failed(), 0, "{report:?}");
    assert_eq!(
        steps_seen,
        vec![
            MigrateStep::Records,
            MigrateStep::Documents,
            MigrateStep::Goals,
            MigrateStep::Profile,
            MigrateStep::Episodic,
            MigrateStep::Content,
        ]
    );

    // Documents: only the one with details, and not the synced namespace.
    let documents = report.step(MigrateStep::Documents).unwrap();
    assert_eq!((documents.read, documents.written), (2, 1));
    let titled = target
        .documents
        .lock()
        .unwrap()
        .get(&("notes".to_string(), "titled".to_string()))
        .cloned()
        .expect("titled document copied");
    assert_eq!(titled.title, "Trip plan");
    assert_eq!(titled.tags, vec!["travel".to_string()]);
    assert_eq!(titled.document_id, "doc-titled");

    // Goals: the target's own first, the same goal not twice, a used id moved.
    let goals = target.goals.lock().unwrap().clone();
    assert_eq!(
        goals
            .items
            .iter()
            .map(|g| (g.id.as_str(), g.text.as_str()))
            .collect::<Vec<_>>(),
        vec![("g1", "Ship the app"), ("g2", "learn rust")]
    );

    // Profile: the newer claim stays.
    let facets = target.facets.lock().unwrap().clone();
    assert_eq!(facets["style/tone"].value, "formal");
    assert_eq!(facets["style/length"].value, "short");

    // Episodic: turn 1 met another turn and moved; every reference followed.
    let turns = target.turns.lock().unwrap().clone();
    let moved = FRESH_IDS + 1;
    assert_eq!(turns[&moved].content, "plan the trip");
    assert_eq!(turns[&1].content, "an older conversation");
    assert_eq!(turns[&2].content, "book flights");
    let segment = target.segments.lock().unwrap()["seg-1"].clone();
    assert_eq!(
        (segment.start_episodic_id, segment.end_episodic_id),
        (moved, Some(2))
    );
    let event = target.events.lock().unwrap()["ev-1"].clone();
    assert_eq!(
        event.source_turn_ids.as_deref(),
        Some(format!("[{moved},2]").as_str())
    );
    assert_eq!(target.embeddings.lock().unwrap().len(), 1);

    assert_content_replayed(&target);
    assert_eq!(report.step(MigrateStep::Content).unwrap().read, 4);

    // A second run finds everything in place.
    let puts_before = *target.puts.lock().unwrap();
    let again = copy_all(&source, &target, &options(), |_| {})
        .await
        .expect("rerun");
    for step in [
        MigrateStep::Documents,
        MigrateStep::Goals,
        MigrateStep::Profile,
        MigrateStep::Episodic,
    ] {
        assert_eq!(again.step(step).unwrap().written, 0, "{step} wrote again");
    }
    assert_eq!(*target.puts.lock().unwrap(), puts_before);
    assert_eq!(target.turns.lock().unwrap().len(), 3, "no turn twice");
}

#[tokio::test]
async fn a_step_whose_family_a_side_lacks_is_skipped_with_the_reason() {
    let source = Fake::serving(&[Capability::Goals]);
    let target = Fake::serving(&[Capability::Profile]);
    let report = copy_all(&source, &target, &CopyOptions::default(), |_| {})
        .await
        .expect("copy");
    let reason = |step| {
        report
            .step(step)
            .and_then(|r| r.skipped_because.clone())
            .unwrap_or_default()
    };
    assert_eq!(
        reason(MigrateStep::Goals),
        "the target does not serve goals"
    );
    assert_eq!(
        reason(MigrateStep::Profile),
        "the source does not serve profile"
    );
    assert_eq!(
        reason(MigrateStep::Episodic),
        "the source does not serve episodic_portability"
    );
    assert_eq!(
        reason(MigrateStep::Content),
        "the source does not serve chunks"
    );

    let off = CopyOptions {
        replay_content: false,
        ..CopyOptions::default()
    };
    let report = copy_all(&seeded_source(), &seeded_target(), &off, |_| {})
        .await
        .expect("copy");
    assert!(report
        .step(MigrateStep::Content)
        .unwrap()
        .skipped_because
        .is_some());
}

#[test]
fn turn_references_are_rewritten_in_their_own_encoding() {
    let map: HashMap<i64, i64> = [(1, 10), (10, 20)].into_iter().collect();
    // One lookup each: 1 becomes 10, and the old 10 becomes 20 — never 1 → 20.
    assert_eq!(remap_turn_ids("[1,10,3]", &map), "[10,20,3]");
    assert_eq!(remap_turn_ids("1, 10,3", &map), "10,20,3");
    assert_eq!(remap_turn_ids("turns 1-3", &map), "turns 1-3");
    assert_eq!(remap_turn_ids("", &map), "");
    assert_eq!(remap_turn_ids("[1]", &HashMap::new()), "[1]");
}

#[test]
fn a_replayed_sources_provider_and_taint_come_from_its_id() {
    assert_eq!(
        data_source(SourceKind::Email, "gmail:me|t1"),
        DataSource::Gmail
    );
    assert_eq!(
        data_source(SourceKind::Email, "outlook:t1"),
        DataSource::OtherEmail
    );
    assert_eq!(
        data_source(SourceKind::Chat, "telegram:42"),
        DataSource::Telegram
    );
    assert_eq!(
        data_source(SourceKind::Chat, "slack:C1"),
        DataSource::Conversation
    );
    assert_eq!(
        data_source(SourceKind::Document, "https://x.y"),
        DataSource::WebPage
    );
    assert_eq!(
        data_source(SourceKind::Document, "report.pdf"),
        DataSource::Upload
    );
    assert_eq!(taint_of("conversations:agent"), MemoryTaint::Internal);
    assert_eq!(taint_of("gmail:me|t1"), MemoryTaint::ExternalSync);
}

/// The embedded engine lists namespaces in their stored spelling, so its
/// synced `source:gmail:…` items come back as `source_gmail_…`. The step
/// still leaves those, and hosted memory's `sources/…`, to the replay rather
/// than re-putting every synced item whole, and keeps a namespace that only
/// looks alike.
#[tokio::test]
async fn synced_namespaces_are_skipped_in_the_spelling_the_engine_lists() {
    let source = Fake::serving(&EVERYTHING);
    {
        let mut documents = source.documents.lock().unwrap();
        for d in [
            document("notes", "titled", "Trip plan", &["travel"]),
            document("source_gmail_ca_x1", "thread-1", "Re: invoice", &[]),
            document("sources/email", "thread-2", "Re: invoice", &[]),
            document("source-notes", "kept", "Not a sync", &[]),
        ] {
            documents.insert((d.namespace.clone(), d.key.clone()), d);
        }
    }
    let target = Fake::serving(&EVERYTHING);
    let report = copy_all(&source, &target, &options(), |_| {})
        .await
        .expect("copy");

    let documents = report.step(MigrateStep::Documents).unwrap();
    assert_eq!((documents.read, documents.written), (2, 2), "{report:?}");
    let copied: Vec<String> = target
        .documents
        .lock()
        .unwrap()
        .keys()
        .map(|(namespace, _)| namespace.clone())
        .collect();
    assert_eq!(
        copied,
        vec!["notes".to_string(), "source-notes".to_string()]
    );
}
