//! Namespace paths, sanitizing and reach.


use super::*;

fn ns(value: &str) -> Namespace {
    value.parse().unwrap()
}

#[test]
fn parses_and_prints_paths() {
    let writer = ns("team:acme/agent:writer");
    assert_eq!(writer.depth(), 2);
    assert_eq!(writer.to_string(), "team:acme/agent:writer");
    assert_eq!(writer.segments()[0].kind(), SegmentKind::Team);
    assert_eq!(writer.segments()[1].id(), "writer");
    assert_eq!(ns("ws:shared").to_string(), "ws:shared");
    assert!(ns("").is_root());
    assert!(ns(ROOT_LABEL).is_root());
    assert_eq!(Namespace::ROOT.to_string(), ROOT_LABEL);
}

#[test]
fn refuses_malformed_paths() {
    for bad in [
        "agent",
        "robot:x",
        "agent:",
        "agent:a.b",
        "agent:a/",
        "agent:a//agent:b",
    ] {
        assert!(bad.parse::<Namespace>().is_err(), "{bad}");
    }
    let deep = vec!["agent:a"; MAX_DEPTH + 1].join("/");
    assert!(deep.parse::<Namespace>().is_err());
    assert!(Segment::new(SegmentKind::Agent, "x".repeat(MAX_SEGMENT_ID + 1)).is_err());
}

#[test]
fn sanitizes_host_ids_without_collisions() {
    assert_eq!(Namespace::agent("researcher").to_string(), "agent:researcher");
    let dotted = Namespace::agent("a.b");
    let underscored = Namespace::agent("a_b");
    assert_ne!(dotted, underscored);
    assert!(dotted.segments()[0].id().starts_with("a-b-"));
    assert!(dotted.to_string().parse::<Namespace>().is_ok());
    assert_eq!(Namespace::agent("").segments()[0].id(), "_");
    let long = Namespace::agent(&"é".repeat(300));
    assert!(long.segments()[0].id().len() <= MAX_SEGMENT_ID);
    assert!(long.to_string().parse::<Namespace>().is_ok());
}

#[test]
fn walks_the_tree() {
    let scout = ns("agent:researcher/agent:scout");
    assert_eq!(scout.parent(), Some(ns("agent:researcher")));
    assert_eq!(Namespace::ROOT.parent(), None);
    assert_eq!(
        scout.ancestors_and_self(),
        vec![Namespace::ROOT, ns("agent:researcher"), scout.clone()]
    );
    assert!(scout.is_within(&ns("agent:researcher")));
    assert!(scout.is_within(&Namespace::ROOT));
    assert!(!ns("agent:researcher").is_within(&scout));
    assert!(!ns("agent:researchers").is_within(&ns("agent:researcher")));
    let child = ns("team:acme")
        .child(Segment::new(SegmentKind::Agent, "writer").unwrap())
        .unwrap();
    assert_eq!(child, ns("team:acme/agent:writer"));
}

#[test]
fn shared_ancestor_skips_agents() {
    assert_eq!(ns("agent:a/agent:b").shared_ancestor(), Namespace::ROOT);
    assert_eq!(
        ns("team:acme/agent:writer").shared_ancestor(),
        ns("team:acme")
    );
    assert_eq!(ns("team:acme").shared_ancestor(), ns("team:acme"));
    assert_eq!(Namespace::ROOT.shared_ancestor(), Namespace::ROOT);
}

#[test]
fn reach_never_crosses_to_a_sibling() {
    let a = ns("agent:a");
    let b = ns("agent:b");
    let below_a = ns("agent:a/agent:scout");
    let reach = Reach::of(a.clone());
    assert!(reach.admits(&a));
    assert!(reach.admits(&Namespace::ROOT));
    assert!(!reach.admits(&b));
    assert!(!reach.admits(&below_a));
    assert_eq!(reach.nodes(), vec![Namespace::ROOT, a.clone()]);

    let exact = Reach::exact(a.clone());
    assert!(!exact.admits(&Namespace::ROOT));
    assert_eq!(exact.nodes(), vec![a.clone()]);

    let subtree = Reach::subtree(a.clone());
    assert!(subtree.admits(&below_a));
    assert!(!subtree.admits(&Namespace::ROOT));
    assert!(!subtree.admits(&b));

    let root = Reach::default();
    assert!(root.admits(&Namespace::ROOT));
    assert!(!root.admits(&a), "the root does not read its agents");
}

#[test]
fn serializes_as_strings() {
    let reach = Reach::of(ns("team:acme/agent:writer"));
    let json = serde_json::to_value(&reach).unwrap();
    assert_eq!(
        json,
        serde_json::json!({"at": "team:acme/agent:writer", "inherit": true, "descendants": false})
    );
    let back: Reach = serde_json::from_value(serde_json::json!({"at": "agent:x"})).unwrap();
    assert_eq!(back, Reach::of(ns("agent:x")));
    assert!(serde_json::from_value::<Namespace>(serde_json::json!("nope")).is_err());
}
