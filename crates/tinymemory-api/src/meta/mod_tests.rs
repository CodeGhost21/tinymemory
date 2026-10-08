//! Metadata serialisation and defaults.

use super::*;

#[test]
fn a_default_meta_names_the_agent_as_its_source() {
    let meta = MemoryMeta::default();
    assert_eq!(meta.source.kind, SourceKind::Agent);
    assert_eq!(meta.source.id, None);
}

#[test]
fn unset_fields_are_omitted_and_round_trip() {
    let mut meta = MemoryMeta::from_source(SourceKind::Github, Some("src_1".into()));
    meta.repo = Some("owner/name".into());
    meta.tool_call = Some(ToolCallRef {
        name: "grep".into(),
        id: None,
    });
    let json = serde_json::to_value(&meta).expect("serialise");
    assert_eq!(
        json,
        serde_json::json!({
            "repo": "owner/name",
            "tool_call": { "name": "grep" },
            "source": { "kind": "github", "id": "src_1" }
        })
    );
    let back: MemoryMeta = serde_json::from_value(json).expect("deserialise");
    assert_eq!(back, meta);
}

#[test]
fn an_empty_object_deserialises_to_the_default() {
    let meta: MemoryMeta = serde_json::from_str("{}").expect("deserialise");
    assert_eq!(meta, MemoryMeta::default());
}

#[test]
fn source_kind_wire_strings_match_serde() {
    for kind in SourceKind::ALL {
        let json = serde_json::to_value(kind).expect("serialise");
        assert_eq!(json, serde_json::json!(kind.as_str()));
    }
}
