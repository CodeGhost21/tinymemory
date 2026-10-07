//! The legacy and v3 scope layouts: every rendering rule and its inverse.

use super::*;

fn ns(value: &str) -> Namespace {
    value.parse().unwrap()
}

/// The direct wire's v3 layout: paths start at the root.
fn v3() -> ScopeLayout {
    ScopeLayout::v3("user:42", false).unwrap()
}

/// The hosted wire's: paths may carry a tenant prefix.
fn hosted() -> ScopeLayout {
    ScopeLayout::v3("user:42", true).unwrap()
}

#[test]
fn v3_gives_every_kind_a_leaf_below_the_root() {
    let layout = v3();
    let cases = [
        (Namespace::ROOT, ItemKind::Learning, "user:42/app:learnings"),
        (
            Namespace::ROOT,
            ItemKind::Conversation,
            "user:42/app:conversations",
        ),
        (Namespace::ROOT, ItemKind::Document, "user:42/app:documents"),
        (
            ns("ws:main"),
            ItemKind::Conversation,
            "user:42/ws:main/app:conversations",
        ),
        (
            ns("ws:main/agent:coder"),
            ItemKind::Conversation,
            "user:42/ws:main/agent:coder/app:conversations",
        ),
        (
            ns("source:gmail"),
            ItemKind::Document,
            "user:42/app:brain/source:gmail",
        ),
        (
            ns("source:github/project:api"),
            ItemKind::Document,
            "user:42/app:brain/source:github/project:api",
        ),
        // Beliefs CortexDB builds in a source scope read back as learnings
        // of the source node, outside the brain's grouping node.
        (
            ns("source:gmail"),
            ItemKind::Learning,
            "user:42/source:gmail/app:learnings",
        ),
        (
            ns("ws:main/service:newsletter"),
            ItemKind::Learning,
            "user:42/ws:main/app:flows/service:newsletter/app:learnings",
        ),
        (
            ns("ws:main/service:newsletter"),
            ItemKind::Document,
            "user:42/ws:main/app:flows/service:newsletter/app:documents",
        ),
        // No workspace: the workflow sits right below the root.
        (
            ns("service:digest"),
            ItemKind::Learning,
            "user:42/app:flows/service:digest/app:learnings",
        ),
    ];
    for (namespace, kind, path) in cases {
        assert_eq!(layout.path(&namespace, kind), path, "{namespace} {kind:?}");
        assert_eq!(
            layout.parse(path),
            Some((namespace.clone(), kind)),
            "{path}"
        );
        assert_eq!(
            hosted().parse(path),
            Some((namespace.clone(), kind)),
            "{path}"
        );
        assert_eq!(
            hosted().parse(&format!("org:u-tenant/{path}")),
            Some((namespace, kind)),
            "a hosted tenant prefix is tolerated: {path}"
        );
        assert_eq!(
            layout.parse(&format!("org:u-tenant/{path}")),
            None,
            "the direct wire never prefixes: {path}"
        );
    }
}

#[test]
fn a_root_repeated_at_the_start_reads_by_wire() {
    // Direct: no prefix exists, so the repeat is the namespace's own.
    assert_eq!(
        v3().parse("user:42/user:42/app:learnings"),
        Some((ns("user:42"), ItemKind::Learning))
    );
    // Hosted: a tenant prefix spelled like the root is a prefix.
    assert_eq!(
        hosted().parse("user:42/user:42/app:learnings"),
        Some((Namespace::ROOT, ItemKind::Learning))
    );
    assert_eq!(
        hosted().parse("user:42/user:42/ws:main/app:conversations"),
        Some((ns("ws:main"), ItemKind::Conversation))
    );
}

#[test]
fn every_scope_at_or_below_a_node_is_under_one_of_its_prefixes() {
    let layout = v3();
    assert_eq!(layout.node_prefixes(&Namespace::ROOT), ["user:42"]);
    assert_eq!(layout.node_prefixes(&ns("ws:main")), ["user:42/ws:main"]);
    assert_eq!(
        layout.node_prefixes(&ns("ws:main/service:nl")),
        ["user:42/ws:main/app:flows/service:nl"]
    );
    assert_eq!(
        layout.node_prefixes(&ns("source:gmail")),
        ["user:42/source:gmail", "user:42/app:brain/source:gmail"]
    );
    let nodes = [
        "ws:main",
        "ws:main/agent:a",
        "ws:main/service:nl",
        "ws:main/service:nl/agent:w",
        "ws:main/source:files",
        "source:gmail",
        "source:gmail/project:p",
    ];
    for node in nodes {
        let prefixes = layout.node_prefixes(&ns(node));
        for below in nodes
            .iter()
            .filter(|b| **b == node || b.starts_with(&format!("{node}/")))
        {
            for kind in ItemKind::ALL {
                let path = layout.path(&ns(below), kind);
                assert!(
                    prefixes.iter().any(|p| path.starts_with(p.as_str())),
                    "{path} is under no prefix of {node}: {prefixes:?}"
                );
            }
        }
    }
    let legacy = ScopeLayout::default();
    assert_eq!(legacy.node_prefixes(&Namespace::ROOT), ["app:tinymemory"]);
    assert_eq!(
        legacy.node_prefixes(&ns("ws:main")),
        ["app:tinymemory/ws:main"]
    );
}

#[test]
fn v3_reads_back_only_its_own_canonical_scopes() {
    let layout = v3();
    for other in [
        "user:42",
        "user:7/app:learnings",
        "app:tinymemory/app:learnings",
        "user:42/ws:main",
        "user:42/app:brain/ws:main/app:conversations",
        "user:42/app:flows/ws:main/app:learnings",
        "user:42/ws:main/service:newsletter/app:learnings",
        "user:42/source:gmail",
        "user:42/robot:x/app:learnings",
    ] {
        assert_eq!(layout.parse(other), None, "{other}");
    }
}

#[test]
fn legacy_is_the_tinymemory_root_layout() {
    let layout = ScopeLayout::default();
    let writer = ns("team:acme/agent:writer");
    let path = layout.path(&writer, ItemKind::Conversation);
    assert_eq!(
        path,
        "app:tinymemory/team:acme/agent:writer/app:conversations"
    );
    assert_eq!(
        layout.parse(&path),
        Some((writer.clone(), ItemKind::Conversation))
    );
    assert_eq!(layout.parse("user:42/app:learnings"), None);
    assert_eq!(layout.root(), "app:tinymemory");
    assert_eq!(
        layout.node_prefixes(&writer),
        ["app:tinymemory/team:acme/agent:writer"]
    );
}

#[test]
fn a_v3_root_uses_only_hosted_scope_types() {
    assert_eq!(v3().root(), "user:42");
    assert_eq!(
        ScopeLayout::v3(" /org:acme/user:6512ab0f/ ", false)
            .unwrap()
            .root(),
        "org:acme/user:6512ab0f"
    );
    for bad in [
        "",
        "user",
        "user:",
        "user:a.b",
        "kb:policies",
        "global:x",
        "user:42//ws:main",
        "app:tinymemory",
        "user:42/app:tinymemory",
    ] {
        assert!(ScopeLayout::v3(bad, false).is_err(), "{bad}");
    }
}

#[test]
fn a_namespace_repeating_the_root_is_flagged() {
    assert!(v3().repeats_root(&ns("user:42")));
    assert!(v3().repeats_root(&ns("user:42/ws:main")));
    assert!(!v3().repeats_root(&ns("ws:main/user:42")));
    assert!(!v3().repeats_root(&ns("user:7")));
    assert!(!v3().repeats_root(&Namespace::ROOT));
    assert!(!ScopeLayout::default().repeats_root(&ns("user:42")));
    let deep = ScopeLayout::v3("team:a/user:42", false).unwrap();
    assert!(deep.repeats_root(&ns("team:a/user:42/ws:main")));
    assert!(!deep.repeats_root(&ns("team:a")));
}
