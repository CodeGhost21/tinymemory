//! Tests for the v2 envelope: layout, round trip, and foreign events.

use super::*;
use tinymemory_api::{SourceKind, Turn};

fn meta() -> MemoryMeta {
    let mut meta = MemoryMeta::from_source(SourceKind::Folder, Some("notes".into()));
    // A file's name round-trips; its folders never leave the machine (see
    // `local_paths_tests.rs`).
    meta.file_path = Some("a.md".into());
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
    let request = envelopes[1].request(&envelopes[1].encode_checked().unwrap());
    assert_eq!(request["content"]["role"], "assistant");
    assert_eq!(request["scope"], "app:tinymemory/app:conversations");
    assert_eq!(request["modality"], "conversation");
}

#[test]
fn a_recall_rendering_is_read_as_well_as_the_stored_text() {
    let mut envelope = Envelope::for_item(&conversation(), "id").unwrap().remove(0);
    let stored = envelope.encode().unwrap();
    envelope.v = 2;
    assert_eq!(
        Envelope::decode(&format!("[user] {stored}")),
        Some(envelope.clone())
    );
    assert_eq!(Envelope::decode(&stored), Some(envelope));
}

#[test]
fn events_this_crate_did_not_write_are_ignored() {
    assert!(Envelope::decode("just a sentence").is_none());
    assert!(Envelope::decode(r#"{"k":"v1-key","c":"v1 content"}"#).is_none());
    let mut old = Envelope::for_item(&conversation(), "id").unwrap().remove(0);
    old.v = 1;
    assert!(Envelope::decode(&json_of(&old).unwrap()).is_none());
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
    let request = envelope.request(&envelope.encode_checked().unwrap());
    assert_eq!(
        request["context"]["observed_at"],
        "2026-01-02T03:04:05+00:00"
    );
    assert_eq!(request["context"]["labels"][0], labels::item("id"));
    assert_eq!(request["scope"], "app:tinymemory/app:documents");
    let key = request["idempotency_key"].as_str().unwrap();
    assert!(key.starts_with("tm3:") && key.len() <= 64, "{key}");
    assert_eq!(
        request["idempotency_key"],
        envelope.request(&envelope.encode_checked().unwrap())["idempotency_key"],
        "the same body, the same key: a retry is a replay"
    );
    let mut later = envelope.clone();
    later.meta.observed_at = Some("2026-01-03T00:00:00Z".parse().unwrap());
    assert_ne!(
        request["idempotency_key"],
        later.request(&later.encode_checked().unwrap())["idempotency_key"],
        "any change to the body is a new key, never a reused key's 409"
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
    let request = envelope.request(&envelope.encode_checked().unwrap());
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

#[test]
fn an_item_that_opts_out_of_derivation_asks_to_extract_nothing() {
    let mut opted_out = meta();
    opted_out.derive = Some(false);
    let item = StoreItem::document("Run digest: 3 items sent.", opted_out);
    let envelope = Envelope::for_item(&item, &item.fingerprint())
        .unwrap()
        .remove(0);
    let request = envelope.request(&envelope.encode_checked().unwrap());
    assert_eq!(request["directives"], serde_json::json!({ "extract": [] }));

    let plain = StoreItem::document("A note.", meta());
    let envelope = Envelope::for_item(&plain, &plain.fingerprint())
        .unwrap()
        .remove(0);
    let request = envelope.request(&envelope.encode_checked().unwrap());
    assert!(request.get("directives").is_none(), "{request}");
}

/// The event CortexDB would hand back for `request`: its id, content and
/// context as sent.
fn stored(request: &Value) -> Value {
    json!({
        "id": "evt_1",
        "content": request["content"],
        "context": request["context"],
    })
}

#[test]
fn an_event_is_written_as_its_own_text_with_the_envelope_in_labels() {
    let mut meta = meta();
    meta.file_path = Some("/docs/handbook.pdf".into());
    let item = StoreItem::document("Refunds take five days.", meta);
    let envelope = Envelope::for_item(&item, &item.fingerprint())
        .unwrap()
        .remove(0);
    let encoded = envelope.encode_checked().unwrap();
    assert_eq!(encoded.text, "Refunds take five days.", "prose, not JSON");
    let request = envelope.request(&encoded);
    let labels: Vec<&str> = request["context"]["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|label| label.as_str().unwrap())
        .collect();
    assert_eq!(labels[0], labels::item(&item.fingerprint()), "lookup first");
    assert!(labels.contains(&"kind:document"), "{labels:?}");
    assert!(labels.contains(&"file:handbook.pdf"), "{labels:?}");
    assert!(labels.iter().any(|label| label.starts_with("tm:e:00:")));
    assert!(labels.len() <= 64);
    assert!(labels.iter().all(|label| label.len() <= 256), "{labels:?}");
    assert!(!labels.iter().any(|label| label.starts_with("lang:")));

    let decoded = decode_event(&stored(&request)).unwrap();
    assert_eq!(decoded.envelope, envelope);
    let mut read_back = item.clone();
    read_back.meta_mut().file_path = Some("handbook.pdf".into());
    assert_eq!(rebuild(&[decoded.envelope]).unwrap(), read_back);
}

#[test]
fn every_piece_and_turn_round_trips_through_its_labels() {
    for item in [conversation(), long_document()] {
        let envelopes = Envelope::for_item(&item, &item.fingerprint()).unwrap();
        let read: Vec<Envelope> = envelopes
            .iter()
            .map(|envelope| {
                let request = envelope.request(&envelope.encode_checked().unwrap());
                assert!(
                    !request["content"]["text"]
                        .as_str()
                        .unwrap()
                        .starts_with('{')
                );
                decode_event(&stored(&request)).unwrap().envelope
            })
            .collect();
        assert_eq!(read, envelopes);
        assert_eq!(rebuild(&read).unwrap(), item);
    }
}

#[test]
fn a_piece_names_its_pages_and_section_in_readable_labels() {
    let mut envelope = Envelope::for_item(&StoreItem::document("piece", meta()), "id")
        .unwrap()
        .remove(0);
    envelope.chunk = Some(ChunkInfo {
        index: 1,
        count: 3,
        pages: Some([3, 5]),
        section: Some("Billing".into()),
    });
    let labels = envelope.encode_checked().unwrap().labels;
    assert!(labels.contains(&"page:3-5".to_string()), "{labels:?}");
    assert!(
        labels.contains(&"section:Billing".to_string()),
        "{labels:?}"
    );
}

#[test]
fn an_envelope_too_big_for_its_labels_or_with_no_text_is_written_as_v2() {
    let mut big = meta();
    big.tags = (0..2000).map(|n| format!("tag-number-{n:04}")).collect();
    let item = StoreItem::document("Some text.", big);
    let envelope = Envelope::for_item(&item, &item.fingerprint())
        .unwrap()
        .remove(0);
    let encoded = envelope.encode_checked().unwrap();
    assert!(
        encoded.text.starts_with('{'),
        "the whole envelope: {}",
        encoded.text
    );
    assert!(
        !encoded
            .labels
            .iter()
            .any(|label| label.starts_with("tm:e:"))
    );
    let decoded = decode_event(&stored(&envelope.request(&encoded))).unwrap();
    assert_eq!(rebuild(&[decoded.envelope]).unwrap(), item);

    let mut silent = Envelope::for_item(&conversation(), "id").unwrap().remove(0);
    silent.text = String::new();
    let encoded = silent.encode_checked().unwrap();
    assert!(encoded.text.starts_with('{'), "never an empty message text");
    assert_eq!(
        decode_event(&stored(&silent.request(&encoded)))
            .unwrap()
            .envelope
            .text,
        ""
    );
}

#[test]
fn incomplete_or_foreign_part_labels_are_not_an_envelope() {
    let mut tagged = meta();
    tagged.tags = (0..40).map(|n| format!("tag-{n}")).collect();
    let item = StoreItem::document("Some text.", tagged);
    let envelope = Envelope::for_item(&item, "id").unwrap().remove(0);
    let labels = envelope.encode_checked().unwrap().labels;
    let parts: Vec<&str> = labels
        .iter()
        .map(String::as_str)
        .filter(|label| label.starts_with("tm:e:"))
        .collect();
    assert!(parts.len() > 1, "{parts:?}");
    let mut shuffled = parts.clone();
    shuffled.reverse();
    assert_eq!(
        Envelope::from_labels("t", shuffled).unwrap().text,
        "t",
        "order is by number"
    );
    assert!(
        Envelope::from_labels("t", parts[1..].iter().copied()).is_none(),
        "a part missing"
    );
    assert!(
        Envelope::from_labels("t", ["tm:e:00:{\"v\":2}"]).is_none(),
        "not v3"
    );
    assert!(
        Envelope::from_labels("t", ["owner:someone"]).is_none(),
        "no parts"
    );
}

#[test]
fn a_tool_turn_asks_to_extract_nothing_and_other_turns_do_not() {
    let item = StoreItem::Conversation {
        turns: vec![
            Turn::new(Role::User, "Look up the weather."),
            Turn::new(Role::Tool, r#"{"temp_c": 21}"#),
            Turn::new(Role::Assistant, "It is 21 degrees."),
        ],
        meta: meta(),
    };
    let extract: Vec<Value> = Envelope::for_item(&item, "id")
        .unwrap()
        .iter()
        .map(|envelope| envelope.request(&envelope.encode_checked().unwrap())["directives"].clone())
        .collect();
    assert_eq!(
        extract,
        [Value::Null, json!({ "extract": [] }), Value::Null]
    );
}
