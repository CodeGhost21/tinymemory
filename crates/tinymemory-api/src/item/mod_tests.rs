//! Item validation, rendering, fingerprints and serde shape.

use super::*;
use crate::meta::SourceKind;

fn conversation(texts: &[&str]) -> StoreItem {
    StoreItem::Conversation {
        turns: texts
            .iter()
            .enumerate()
            .map(|(i, t)| Turn::new(if i % 2 == 0 { Role::User } else { Role::Assistant }, *t))
            .collect(),
        meta: MemoryMeta::default(),
    }
}

#[test]
fn kinds_follow_the_variant() {
    assert_eq!(
        StoreItem::document("x", MemoryMeta::default()).kind(),
        ItemKind::Document
    );
    assert_eq!(conversation(&["hi"]).kind(), ItemKind::Conversation);
    assert_eq!(
        StoreItem::learning("x", LearningKind::Fact, 0.5, MemoryMeta::default()).kind(),
        ItemKind::Learning
    );
}

#[test]
fn invalid_items_are_refused() {
    let refused = [
        StoreItem::document("  ", MemoryMeta::default()),
        StoreItem::Document {
            title: None,
            body: DocumentBody::Uri("file:///x".into()),
            mime: None,
            meta: MemoryMeta::default(),
        },
        conversation(&[]),
        conversation(&["hi", " "]),
        StoreItem::learning("", LearningKind::Fact, 0.5, MemoryMeta::default()),
        StoreItem::learning("x", LearningKind::Fact, 1.5, MemoryMeta::default()),
        StoreItem::learning("x", LearningKind::Fact, f32::NAN, MemoryMeta::default()),
    ];
    for item in refused {
        assert!(
            matches!(item.validate(), Err(Error::InvalidRequest(_))),
            "{item:?}"
        );
    }
    assert!(conversation(&["hi", "hello"]).validate().is_ok());
}

#[test]
fn rendering_reads_like_the_item() {
    let doc = StoreItem::Document {
        title: Some("Notes".into()),
        body: DocumentBody::Text("body".into()),
        mime: None,
        meta: MemoryMeta::default(),
    };
    assert_eq!(doc.render_text(), "# Notes\n\nbody");
    assert_eq!(conversation(&["hi", "yo"]).render_text(), "user: hi\nassistant: yo");
}

#[test]
fn fingerprints_cover_metadata_and_are_stable() {
    let a = StoreItem::document("same", MemoryMeta::default());
    let b = StoreItem::document("same", MemoryMeta::default());
    let c = StoreItem::document(
        "same",
        MemoryMeta::from_source(SourceKind::Folder, Some("s".into())),
    );
    assert_eq!(a.fingerprint(), b.fingerprint());
    assert_ne!(a.fingerprint(), c.fingerprint());
    assert_eq!(a.fingerprint().len(), 40);
}

#[test]
fn items_serialise_with_a_kind_tag_and_round_trip() {
    let item = StoreItem::learning("tea", LearningKind::Preference, 0.9, MemoryMeta::default());
    let json = serde_json::to_value(&item).expect("serialise");
    assert_eq!(json["kind"], "learning");
    assert_eq!(json["kind"], ItemKind::Learning.as_str());
    let back: StoreItem = serde_json::from_value(json).expect("deserialise");
    assert_eq!(back, item);
}

#[test]
fn meta_mut_edits_in_place() {
    let mut item = conversation(&["hi"]);
    item.meta_mut().thread_id = Some("t".into());
    assert_eq!(item.meta().thread_id.as_deref(), Some("t"));
}

#[test]
fn ids_display_and_convert() {
    let id = ItemId::from("abc");
    assert_eq!(id.to_string(), "abc");
    assert_eq!(id.as_str(), "abc");
    assert_eq!(ItemId::new(String::from("abc")), id);
    assert_eq!(serde_json::to_value(&id).expect("json"), serde_json::json!("abc"));
}

#[test]
fn wire_strings_match_serde() {
    for kind in ItemKind::ALL {
        assert_eq!(serde_json::to_value(kind).expect("json"), kind.as_str());
    }
    for role in [Role::User, Role::Assistant, Role::System, Role::Tool] {
        assert_eq!(serde_json::to_value(role).expect("json"), role.as_str());
    }
    for kind in [
        LearningKind::Preference,
        LearningKind::Fact,
        LearningKind::Procedure,
        LearningKind::Correction,
        LearningKind::Other,
    ] {
        assert_eq!(serde_json::to_value(kind).expect("json"), kind.as_str());
    }
}
