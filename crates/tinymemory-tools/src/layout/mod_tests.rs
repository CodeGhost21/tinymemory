//! Layout nodes, filters and brain source ids.

use super::*;

#[test]
fn places_every_part_below_the_root() {
    let layout = MemoryLayout::new("team:acme".parse().unwrap()).unwrap();
    assert_eq!(
        layout.brain(&BrainSource::Pdf).unwrap().to_string(),
        "team:acme/source:pdf"
    );
    assert_eq!(
        layout.conversations("coder-42").unwrap().to_string(),
        "team:acme/agent:coder-42"
    );
    assert_eq!(layout.learnings().to_string(), "team:acme");
    let odd = layout.conversations("agent 7").unwrap();
    assert!(odd.to_string().starts_with("team:acme/agent:agent-7-"));
}

#[test]
fn filters_read_their_scope_only() {
    let layout = MemoryLayout::default();
    let pdf = layout.brain_filter(Some(&BrainSource::Pdf));
    let pdf_reach = pdf.reach.clone().unwrap();
    assert!(pdf_reach.admits(&Namespace::source("pdf")));
    assert!(!pdf_reach.admits(&Namespace::source("notion")));
    assert!(!pdf_reach.admits(&Namespace::ROOT));
    assert_eq!(pdf.kinds, [ItemKind::Document]);

    let brain = layout.brain_filter(None).reach.unwrap();
    assert!(brain.admits(&Namespace::source("notion")));

    let support = layout.conversations_filter(Some("support")).reach.unwrap();
    assert!(support.admits(&Namespace::agent("support")));
    assert!(!support.admits(&Namespace::agent("coder")));
    let team = layout.conversations_filter(None);
    assert!(team.reach.unwrap().admits(&Namespace::agent("coder")));
    assert_eq!(team.kinds, [ItemKind::Conversation]);

    assert_eq!(layout.learnings_filter().kinds, [ItemKind::Learning]);
    assert!(layout.holistic_filter().kinds.is_empty());
}

#[test]
fn refuses_a_root_with_no_room_below() {
    let deep: Namespace = ["agent:a"; 8].join("/").parse().unwrap();
    assert!(matches!(MemoryLayout::new(deep), Err(Error::InvalidRequest(_))));
    let deepest_allowed: Namespace = ["agent:a"; 7].join("/").parse().unwrap();
    let layout = MemoryLayout::new(deepest_allowed).unwrap();
    assert_eq!(layout.brain(&BrainSource::Web).unwrap().depth(), 8);
}

#[test]
fn source_ids_round_trip() {
    for source in [
        BrainSource::Pdf,
        BrainSource::Markdown,
        BrainSource::Notion,
        BrainSource::Github,
        BrainSource::Web,
        BrainSource::Other("confluence".into()),
    ] {
        let id = source.to_string();
        assert_eq!(id.parse::<BrainSource>().unwrap(), source);
        let json = serde_json::to_value(&source).unwrap();
        assert_eq!(json, serde_json::json!(id));
        assert_eq!(serde_json::from_value::<BrainSource>(json).unwrap(), source);
    }
    assert_eq!("MD".parse::<BrainSource>().unwrap(), BrainSource::Markdown);
    assert!(" ".parse::<BrainSource>().is_err());
    assert_eq!(BrainSource::Github.source_kind(), SourceKind::Github);
}
