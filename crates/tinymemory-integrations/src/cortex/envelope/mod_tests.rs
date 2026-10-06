//! Tests for the v2 envelope: layout, round trip, and foreign events.

use super::*;
use tinymemory_api::{SourceKind, Turn};

fn meta() -> MemoryMeta {
    let mut meta = MemoryMeta::from_source(SourceKind::Folder, Some("notes".into()));
    meta.file_path = Some("/notes/a.md".into());
    meta
}

fn conversation() -> StoreItem {
    StoreItem::Conversation {
        turns: vec![
            Turn::new(Role::User, "hello"),
            Turn {
                role: Role::Assistant,
                text: "hi there".into(),
                at: None,
                tool_calls: vec![ToolCallRef {
                    name: "search".into(),
                    id: Some("c1".into()),
                }],
            },
        ],
        meta: meta(),
    }
}

#[test]
fn every_kind_round_trips_through_its_events() {
    let items = [
        StoreItem::Document {
            title: Some("Title".into()),
            body: DocumentBody::Text("body".into()),
            mime: Some("text/markdown".into()),
            meta: meta(),
        },
        StoreItem::Learning {
            text: "prefers tabs".into(),
            kind: LearningKind::Preference,
            confidence: 0.7,
            evidence: Some("said so".into()),
            meta: meta(),
        },
        conversation(),
    ];
    for item in items {
        let id = item.fingerprint();
        let envelopes = Envelope::for_item(&item, &id).unwrap();
        let decoded: Vec<Envelope> = envelopes
            .iter()
            .map(|e| Envelope::decode(&e.encode().unwrap()).unwrap())
            .collect();
        let rebuilt = rebuild(&decoded).unwrap();
        assert_eq!(rebuilt, item);
        assert_eq!(
            rebuilt.fingerprint(),
            id,
            "identity survives the round trip"
        );
    }
}

#[test]
fn a_conversation_is_one_event_per_turn_in_order() {
    let item = conversation();
    let envelopes = Envelope::for_item(&item, "id").unwrap();
    assert_eq!(envelopes.len(), 2);
    let turns: Vec<_> = envelopes
        .iter()
        .map(|e| e.turn.as_ref().map(|t| (t.index, t.count)))
        .collect();
    assert_eq!(turns, vec![Some((0, 2)), Some((1, 2))]);
    let request = envelopes[1].request("x");
    assert_eq!(request["content"]["role"], "assistant");
    assert_eq!(request["scope"], "app:tinymemory/app:conversations");
    assert_eq!(request["modality"], "conversation");
}

#[test]
fn a_recall_rendering_is_read_as_well_as_the_stored_text() {
    let envelope = &Envelope::for_item(&conversation(), "id").unwrap()[0];
    let stored = envelope.encode().unwrap();
    assert_eq!(
        Envelope::decode(&format!("[user] {stored}")).as_ref(),
        Some(envelope)
    );
    assert_eq!(Envelope::decode(&stored).as_ref(), Some(envelope));
}

#[test]
fn events_this_crate_did_not_write_are_ignored() {
    assert!(Envelope::decode("just a sentence").is_none());
    assert!(Envelope::decode(r#"{"k":"v1-key","c":"v1 content"}"#).is_none());
    let mut old = Envelope::for_item(&conversation(), "id").unwrap().remove(0);
    old.v = 1;
    assert!(Envelope::decode(&old.encode().unwrap()).is_none());
    assert!(decode_event(&json!({ "id": "e", "content": { "text": "plain" } })).is_none());
}

#[test]
fn rebuilding_drops_repeated_turns_and_orders_by_index() {
    let envelopes = Envelope::for_item(&conversation(), "id").unwrap();
    let shuffled = vec![
        envelopes[1].clone(),
        envelopes[0].clone(),
        envelopes[1].clone(),
    ];
    assert_eq!(rebuild(&shuffled), Some(conversation()));
}

#[test]
fn observed_at_and_labels_reach_the_event_context() {
    let mut meta = meta();
    meta.observed_at = Some("2026-01-02T03:04:05Z".parse().unwrap());
    let item = StoreItem::document("text", meta);
    let envelope = &Envelope::for_item(&item, "id").unwrap()[0];
    let request = envelope.request("payload");
    assert_eq!(
        request["context"]["observed_at"],
        "2026-01-02T03:04:05+00:00"
    );
    assert_eq!(request["context"]["labels"][0], labels::item("id"));
    assert_eq!(request["scope"], "app:tinymemory/app:documents");
    assert_ne!(
        request["idempotency_key"],
        envelope.request("payload")["idempotency_key"],
        "every write mints a fresh key"
    );
}

#[test]
fn scopes_nest_kinds_under_their_namespace_node() {
    let writer: Namespace = "team:acme/agent:writer".parse().unwrap();
    assert_eq!(
        scope_path(&Namespace::ROOT, ItemKind::Learning),
        "app:tinymemory/app:learnings",
        "the root keeps the original layout"
    );
    let path = scope_path(&writer, ItemKind::Conversation);
    assert_eq!(
        path,
        "app:tinymemory/team:acme/agent:writer/app:conversations"
    );
    assert_eq!(
        parse_scope(&path),
        Some((writer.clone(), ItemKind::Conversation))
    );
    assert_eq!(
        parse_scope(&format!("org:t1/{path}")),
        Some((writer, ItemKind::Conversation)),
        "a tenant prefix is skipped"
    );
    assert_eq!(
        parse_scope("app:tinymemory/app:documents"),
        Some((Namespace::ROOT, ItemKind::Document))
    );
    for other in [
        "app:other/app:documents",
        "app:tinymemory",
        "app:tinymemory/agent:x",
        "app:tinymemory/robot:x/app:documents",
    ] {
        assert_eq!(parse_scope(other), None, "{other}");
    }
}

#[test]
fn a_brain_source_is_a_cortex_source_scope() {
    let pdf: Namespace = "team:acme/source:pdf".parse().unwrap();
    let path = scope_path(&pdf, ItemKind::Document);
    assert_eq!(path, "app:tinymemory/team:acme/source:pdf/app:documents");
    assert_eq!(parse_scope(&path), Some((pdf, ItemKind::Document)));
}

#[test]
fn an_item_is_written_to_its_namespace_scope() {
    let meta = MemoryMeta {
        namespace: Namespace::agent("researcher"),
        ..MemoryMeta::default()
    };
    let item = StoreItem::document("notes", meta);
    let id = item.fingerprint();
    let envelope = Envelope::for_item(&item, &id).unwrap().remove(0);
    let request = envelope.request(&envelope.encode().unwrap());
    assert_eq!(
        request["scope"],
        "app:tinymemory/agent:researcher/app:documents"
    );
}

/// A document long enough to be written as several pieces.
fn long_document() -> StoreItem {
    let body: String = (1..=30)
        .map(|n| {
            format!(
                "# Part {n}\n\n{}\n",
                "Body text for this part. ".repeat(600)
            )
        })
        .collect();
    StoreItem::Document {
        title: Some("Long".into()),
        body: DocumentBody::Text(body),
        mime: None,
        meta: meta(),
    }
}

#[test]
fn a_chunked_document_rebuilds_from_pieces_in_any_order_each_once() {
    let item = long_document();
    let id = item.fingerprint();
    let mut pieces = Envelope::for_item(&item, &id).unwrap();
    assert!(pieces.len() >= 2, "{} pieces", pieces.len());
    assert!(pieces.iter().all(|piece| piece.chunk.is_some()));

    pieces.reverse();
    let duplicated = pieces[1].clone();
    pieces.push(duplicated);
    let rebuilt = rebuild(&pieces).unwrap();
    assert_eq!(
        rebuilt, item,
        "shuffled and duplicated pieces give the body"
    );
    assert_eq!(rebuilt.fingerprint(), id);

    let one = rebuild(std::slice::from_ref(&pieces[0])).unwrap();
    let StoreItem::Document {
        body: DocumentBody::Text(text),
        ..
    } = one
    else {
        panic!("a document");
    };
    assert_eq!(text, pieces[0].text, "one piece alone gives that piece");
}

#[test]
fn a_document_without_pieces_rebuilds_from_its_first_envelope() {
    let item = StoreItem::document("Short note.", meta());
    let id = item.fingerprint();
    let envelopes = Envelope::for_item(&item, &id).unwrap();
    assert_eq!(envelopes.len(), 1);
    assert_eq!(envelopes[0].chunk, None);
    assert!(!envelopes[0].encode().unwrap().contains("\"chunk\""));
    assert_eq!(rebuild(&envelopes).unwrap(), item);
}

#[test]
fn metadata_that_leaves_no_room_for_a_piece_keeps_a_fitting_document_whole() {
    // Metadata just under the limit: no room for a piece's envelope, yet a
    // short document still fits whole.
    let mut near = meta();
    near.tags = vec!["t".repeat(chunks::MAX_EVENT_TEXT_BYTES - 2_000)];
    let fits = StoreItem::document("Short note.", near);
    let id = fits.fingerprint();
    let envelopes = Envelope::for_item(&fits, &id).unwrap();
    assert_eq!(envelopes.len(), 1, "no pieces are made up");
    assert_eq!(envelopes[0].chunk, None);
    envelopes[0].encode_checked().unwrap();
}

#[test]
fn a_document_that_cannot_fit_or_be_split_is_refused_when_laid_out() {
    let mut huge = meta();
    huge.tags = vec!["t".repeat(chunks::MAX_EVENT_TEXT_BYTES)];
    for body in ["Short note.".to_string(), "x".repeat(400_000)] {
        let item = StoreItem::document(body, huge.clone());
        let id = item.fingerprint();
        let refused = Envelope::for_item(&item, &id);
        assert!(
            matches!(&refused, Err(Error::InvalidRequest(message)) if message.contains("1 MiB")),
            "{refused:?}"
        );
    }
}

#[test]
fn a_whitespace_only_document_keeps_its_text() {
    for body in ["   \n\n  ", "\u{c}\u{c}", " \u{c} \n"] {
        let item = StoreItem::document(body, meta());
        let id = item.fingerprint();
        let envelopes = Envelope::for_item(&item, &id).unwrap();
        assert_eq!(envelopes.len(), 1, "{body:?}");
        assert_eq!(rebuild(&envelopes).unwrap(), item, "{body:?}");
    }
}

#[test]
fn a_whole_read_needs_every_piece_of_one_agreed_layout() {
    let item = long_document();
    let pieces = Envelope::for_item(&item, "id").unwrap();
    assert!(pieces.len() > 1);
    assert_eq!(rebuild_whole(&pieces), Some(item.clone()));
    assert_eq!(rebuild_whole(&pieces[1..]), None, "a piece missing");

    let relaid = |edit: &dyn Fn(&mut ChunkInfo)| {
        let mut changed = pieces.clone();
        edit(changed[0].chunk.as_mut().unwrap());
        rebuild_whole(&changed)
    };
    assert_eq!(relaid(&|chunk| chunk.count += 1), None, "counts disagree");
    assert_eq!(
        relaid(&|chunk| chunk.index = 99),
        None,
        "an index past the count"
    );
    let mut zero = pieces[..1].to_vec();
    zero[0].chunk = Some(ChunkInfo {
        index: 0,
        count: 0,
        pages: None,
        section: None,
    });
    assert_eq!(rebuild_whole(&zero), None, "a zero count");

    let mut whole = Envelope::for_item(&StoreItem::document("x", meta()), "id")
        .unwrap()
        .remove(0);
    whole.text = match &item {
        StoreItem::Document {
            body: DocumentBody::Text(text),
            ..
        } => text.clone(),
        _ => unreachable!("a document"),
    };
    whole.title.clone_from(&pieces[0].title);
    whole.meta = pieces[0].meta.clone();
    let mixed = vec![pieces[1].clone(), whole];
    assert_eq!(
        rebuild_whole(&mixed),
        Some(item),
        "the same item written whole before chunking reads as that body"
    );
}
