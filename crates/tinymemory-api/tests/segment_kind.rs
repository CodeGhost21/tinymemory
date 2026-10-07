//! `SegmentKind` from a host's side: it is non-exhaustive, so a host's match
//! carries a wildcard arm, and every kind still has its path prefix.
//!
//! This file compiles outside the crate, where `#[non_exhaustive]` applies.
//! Were the attribute dropped, the wildcard arm below would be unreachable,
//! which `clippy --all-targets -- -D warnings` refuses: the contract is
//! pinned at compile time.

use tinymemory_api::{Namespace, SegmentKind};

/// A host naming each kind it knows, with the wildcard arm a new kind needs.
fn label(kind: SegmentKind) -> &'static str {
    match kind {
        SegmentKind::Agent => "agent",
        SegmentKind::Team => "team",
        SegmentKind::User => "user",
        SegmentKind::Workspace => "workspace",
        SegmentKind::Project => "project",
        SegmentKind::Source => "source",
        SegmentKind::Service => "service",
        _ => "other",
    }
}

#[test]
fn a_host_matches_every_kind_with_a_wildcard_arm() {
    let path: Namespace = "team:a/user:b/ws:c/project:d/source:e/agent:f/service:g"
        .parse()
        .expect("every kind parses");
    let labels: Vec<&str> = path.segments().iter().map(|s| label(s.kind())).collect();
    assert_eq!(
        labels,
        [
            "team",
            "user",
            "workspace",
            "project",
            "source",
            "agent",
            "service"
        ]
    );
    let prefixes: Vec<&str> = path.segments().iter().map(|s| s.kind().as_str()).collect();
    assert_eq!(
        prefixes,
        [
            "team", "user", "ws", "project", "source", "agent", "service"
        ]
    );
}
