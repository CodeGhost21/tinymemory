//! The legacy and v3 scope layouts: every rendering rule and its inverse.

use super::*;

fn ns(value: &str) -> Namespace {
    value.parse().unwrap()
}

fn v3() -> ScopeLayout {
    ScopeLayout::v3("user:42").unwrap()
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
            layout.parse(&format!("org:u-tenant/{path}")),
            Some((namespace, kind)),
            "a tenant prefix is tolerated: {path}"
        );
    }
}

#[test]
fn a_tenant_prefix_spelled_like_the_root_is_not_the_namespace() {
    assert_eq!(
        v3().parse("user:42/user:42/app:learnings"),
        Some((Namespace::ROOT, ItemKind::Learning))
    );
    assert_eq!(
        v3().parse("user:42/user:42/ws:main/app:conversations"),
        Some((ns("ws:main"), ItemKind::Conversation))
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
        layout.node_prefix(&writer),
        "app:tinymemory/team:acme/agent:writer"
    );
    assert_eq!(layout.node_prefix(&Namespace::ROOT), "app:tinymemory");
    assert_eq!(v3().node_prefix(&writer), "user:42");
}

#[test]
fn a_v3_root_uses_only_hosted_scope_types() {
    assert_eq!(v3().root(), "user:42");
    assert_eq!(
        ScopeLayout::v3(" /org:acme/user:6512ab0f/ ")
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
        assert!(ScopeLayout::v3(bad).is_err(), "{bad}");
    }
}
