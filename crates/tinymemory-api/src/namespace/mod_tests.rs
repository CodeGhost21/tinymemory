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
    for kind in [
        "agent", "team", "user", "ws", "project", "source", "service",
    ] {
        let segment = ns(&format!("{kind}:x")).segments()[0].clone();
        assert_eq!(segment.kind().as_str(), kind);
    }
    assert!(ns("").is_root());
    assert!(ns(ROOT_LABEL).is_root());
    assert_eq!(Namespace::ROOT.to_string(), ROOT_LABEL);
}

#[test]
fn a_service_node_prints_parses_and_serializes() {
    let flow = ns("ws:main/service:newsletter");
    assert_eq!(flow.segments()[1].kind(), SegmentKind::Service);
    assert_eq!(flow.segments()[1].id(), "newsletter");
    assert_eq!(flow.to_string(), "ws:main/service:newsletter");
    let json = serde_json::to_string(&flow).unwrap();
    assert_eq!(json, r#""ws:main/service:newsletter""#);
    assert_eq!(serde_json::from_str::<Namespace>(&json).unwrap(), flow);
    assert_eq!(
        serde_json::to_string(&SegmentKind::Service).unwrap(),
        r#""service""#
    );
    let sanitized = Segment::sanitized(SegmentKind::Service, "flow 7");
    assert!(sanitized.to_string().starts_with("service:flow-7-"));
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
    let deep = ["agent:a"; MAX_DEPTH + 1].join("/");
    assert!(deep.parse::<Namespace>().is_err());
    assert!(Segment::new(SegmentKind::Agent, "x".repeat(MAX_SEGMENT_ID + 1)).is_err());
}

#[test]
fn sanitizes_host_ids_without_collisions() {
    assert_eq!(
        Namespace::agent("researcher").to_string(),
        "agent:researcher"
    );
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
    assert_eq!(
        scout.ancestors_and_self(),
        vec![Namespace::ROOT, ns("agent:researcher"), scout.clone()]
    );
    assert!(scout.is_within(&ns("agent:researcher")));
    assert!(scout.is_within(&Namespace::ROOT));
    assert!(!ns("agent:researcher").is_within(&scout));
    assert!(!ns("agent:researchers").is_within(&ns("agent:researcher")));
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
fn a_subtree_read_never_enters_a_service_below_it() {
    let flow = ns("ws:main/service:newsletter");
    let inside = ns("ws:main/service:newsletter/agent:writer");
    let chat = ns("ws:main/agent:assistant");
    let top_level_flow = ns("service:digest");

    for above in [
        Reach::subtree(Namespace::ROOT),
        Reach::subtree(ns("ws:main")),
    ] {
        assert!(above.admits(&chat));
        assert!(!above.admits(&flow), "{above:?} entered the sandbox");
        assert!(!above.admits(&inside), "{above:?} entered the sandbox");
    }
    assert!(!Reach::subtree(Namespace::ROOT).admits(&top_level_flow));

    // A reach at the service, or inside it, reads it as usual.
    assert!(Reach::exact(flow.clone()).admits(&flow));
    assert!(Reach::subtree(flow.clone()).admits(&inside));
    assert!(Reach::of(inside.clone()).admits(&flow));
    // A service nested in a service is a sandbox of its own.
    assert!(!Reach::subtree(flow.clone()).admits(&ns("ws:main/service:newsletter/service:sub")));
}

#[test]
fn no_reach_reads_as_the_root_subtree() {
    let flow = ns("ws:main/service:newsletter");
    let chat = ns("ws:main/agent:assistant");
    assert!(Reach::admitted_by(None, &Namespace::ROOT));
    assert!(Reach::admitted_by(None, &chat));
    assert!(!Reach::admitted_by(None, &flow));
    assert!(Reach::admitted_by(Some(&Reach::exact(flow.clone())), &flow));
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

#[test]
fn builds_source_and_child_nodes() {
    let pdf = Namespace::source("pdf");
    assert_eq!(pdf.to_string(), "source:pdf");
    assert_eq!(pdf.segments()[0].kind(), SegmentKind::Source);
    assert_eq!(ns("source:pdf"), pdf);

    let team = ns("team:acme");
    let child = team
        .child(Segment::sanitized(SegmentKind::Source, "notion export"))
        .unwrap();
    assert_eq!(child.depth(), 2);
    assert!(
        child
            .to_string()
            .starts_with("team:acme/source:notion-export-")
    );
    assert!(Reach::subtree(team).admits(&child));
}

#[test]
fn refuses_a_child_past_the_depth_limit() {
    let deep = ns(&["agent:a"; MAX_DEPTH].join("/"));
    let error = deep
        .child(Segment::sanitized(SegmentKind::Agent, "b"))
        .unwrap_err();
    assert!(matches!(error, Error::InvalidRequest(_)), "{error}");
}

#[test]
fn a_reach_inside_a_subtree_is_within_it() {
    let team = Reach::subtree(ns("team:acme"));
    assert!(team.within(&team));
    assert!(Reach::subtree(ns("team:acme/agent:writer")).within(&team));
    assert!(Reach::exact(ns("team:acme/agent:writer")).within(&team));
    assert!(Reach::exact(ns("team:acme")).within(&team));
    // Everything is within the root's subtree, except a service sandbox.
    let all = Reach::subtree(Namespace::ROOT);
    assert!(Reach::of(ns("team:acme/agent:writer")).within(&all));
    assert!(Reach::subtree(ns("team:acme")).within(&all));
    assert!(!Reach::exact(ns("service:flows")).within(&all));
}

#[test]
fn a_reach_leaving_a_subtree_is_not_within_it() {
    let team = Reach::subtree(ns("team:acme"));
    // A sibling, the root, an ancestor's subtree.
    assert!(!Reach::subtree(ns("team:other")).within(&team));
    assert!(!Reach::exact(Namespace::ROOT).within(&team));
    assert!(!Reach::subtree(Namespace::ROOT).within(&team));
    // Inheriting reads the root above the subtree's top.
    assert!(!Reach::of(ns("team:acme/agent:writer")).within(&team));
    // A service sandbox below the subtree is not read by it.
    assert!(!Reach::exact(ns("team:acme/service:flows")).within(&team));
}

#[test]
fn descendants_need_descendants_and_a_node_at_or_below() {
    let agent = Reach::of(ns("team:acme/agent:writer"));
    assert!(Reach::exact(ns("team:acme")).within(&agent));
    assert!(Reach::of(ns("team:acme")).within(&agent));
    // The team's subtree holds every other member's memory.
    assert!(!Reach::subtree(ns("team:acme")).within(&agent));
    assert!(!Reach::subtree(ns("team:acme/agent:writer")).within(&agent));
    assert!(!Reach::exact(ns("team:acme/agent:editor")).within(&agent));
}
