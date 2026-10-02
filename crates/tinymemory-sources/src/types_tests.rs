//! Tests for source type contracts, serde wire strings, and validation.

use super::*;

#[test]
fn source_kind_round_trips_via_serde() {
    for kind in [
        SourceKind::Composio,
        SourceKind::Conversation,
        SourceKind::Folder,
        SourceKind::GithubRepo,
        SourceKind::TwitterQuery,
        SourceKind::RssFeed,
        SourceKind::WebPage,
    ] {
        let json = serde_json::to_string(&kind).unwrap();
        let decoded: SourceKind = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, kind);
    }
}

#[test]
fn source_kind_as_str_matches_wire_strings() {
    assert_eq!(SourceKind::Composio.as_str(), "composio");
    assert_eq!(SourceKind::Conversation.as_str(), "conversation");
    assert_eq!(SourceKind::Folder.as_str(), "folder");
    assert_eq!(SourceKind::GithubRepo.as_str(), "github_repo");
    assert_eq!(SourceKind::TwitterQuery.as_str(), "twitter_query");
    assert_eq!(SourceKind::RssFeed.as_str(), "rss_feed");
    assert_eq!(SourceKind::WebPage.as_str(), "web_page");
}

#[test]
fn validate_composio_requires_toolkit_and_connection_id() {
    let entry = MemorySourceEntry {
        id: "src_1".into(),
        kind: SourceKind::Composio,
        label: "Gmail".into(),
        enabled: true,
        toolkit: Some("gmail".into()),
        connection_id: None,
        ..default_entry()
    };
    assert!(entry.validate().is_err());

    let valid = MemorySourceEntry {
        connection_id: Some("cmp_123".into()),
        ..entry
    };
    assert!(valid.validate().is_ok());
}

#[test]
fn validate_folder_requires_path() {
    let entry = MemorySourceEntry {
        id: "src_2".into(),
        kind: SourceKind::Folder,
        label: "Notes".into(),
        enabled: true,
        path: None,
        ..default_entry()
    };
    assert!(entry.validate().is_err());
}

#[test]
fn validate_github_requires_url() {
    let entry = MemorySourceEntry {
        id: "src_3".into(),
        kind: SourceKind::GithubRepo,
        label: "Repo".into(),
        enabled: true,
        url: Some("https://github.com/org/repo".into()),
        ..default_entry()
    };
    assert!(entry.validate().is_ok());
}

#[test]
fn validate_twitter_requires_query() {
    let entry = MemorySourceEntry {
        id: "src_tw".into(),
        kind: SourceKind::TwitterQuery,
        label: "Tweets".into(),
        enabled: true,
        ..default_entry()
    };
    assert!(entry.validate().is_err());
}

#[test]
fn validate_rss_and_web_page_require_url() {
    let rss = MemorySourceEntry {
        id: "src_rss".into(),
        kind: SourceKind::RssFeed,
        label: "Feed".into(),
        enabled: true,
        url: None,
        ..default_entry()
    };
    assert!(rss.validate().is_err());

    let web = MemorySourceEntry {
        id: "src_web".into(),
        kind: SourceKind::WebPage,
        label: "Page".into(),
        enabled: true,
        url: Some("https://example.com".into()),
        ..default_entry()
    };
    assert!(web.validate().is_ok());
}

#[test]
fn validate_conversation_needs_only_id_and_label() {
    let entry = MemorySourceEntry {
        id: "src_conv".into(),
        kind: SourceKind::Conversation,
        label: "Agent Conversations".into(),
        enabled: true,
        ..default_entry()
    };
    assert!(entry.validate().is_ok());
}

#[test]
fn validate_conversation_fails_with_empty_id() {
    let entry = MemorySourceEntry {
        id: "".into(),
        kind: SourceKind::Conversation,
        label: "Convos".into(),
        enabled: true,
        ..default_entry()
    };
    assert!(entry.validate().is_err());
}

#[test]
fn validate_conversation_fails_with_empty_label() {
    let entry = MemorySourceEntry {
        id: "src_conv".into(),
        kind: SourceKind::Conversation,
        label: "".into(),
        enabled: true,
        ..default_entry()
    };
    assert!(entry.validate().is_err());
}

#[test]
fn conversation_kind_serializes_to_snake_case() {
    let json = serde_json::to_string(&SourceKind::Conversation).unwrap();
    assert_eq!(json, "\"conversation\"");
}

#[test]
fn content_type_serializes_to_snake_case() {
    assert_eq!(
        serde_json::to_string(&ContentType::Markdown).unwrap(),
        "\"markdown\""
    );
    assert_eq!(
        serde_json::to_string(&ContentType::Html).unwrap(),
        "\"html\""
    );
    assert_eq!(
        serde_json::to_string(&ContentType::Plaintext).unwrap(),
        "\"plaintext\""
    );
}

#[test]
fn toml_round_trip() {
    let entry = MemorySourceEntry {
        id: "src_1".into(),
        kind: SourceKind::Folder,
        label: "My notes".into(),
        enabled: true,
        path: Some("/tmp/notes".into()),
        glob: Some("**/*.md".into()),
        ..default_entry()
    };
    let toml_str = toml::to_string_pretty(&entry).unwrap();
    let decoded: MemorySourceEntry = toml::from_str(&toml_str).unwrap();
    assert_eq!(decoded.id, "src_1");
    assert_eq!(decoded.kind, SourceKind::Folder);
    assert_eq!(decoded.path.as_deref(), Some("/tmp/notes"));
}

#[test]
fn conversation_toml_round_trip() {
    let entry = MemorySourceEntry {
        id: "src_conv".into(),
        kind: SourceKind::Conversation,
        label: "Conversations".into(),
        enabled: true,
        ..default_entry()
    };
    let toml_str = toml::to_string_pretty(&entry).unwrap();
    let decoded: MemorySourceEntry = toml::from_str(&toml_str).unwrap();
    assert_eq!(decoded.id, "src_conv");
    assert_eq!(decoded.kind, SourceKind::Conversation);
    assert_eq!(decoded.label, "Conversations");
    assert!(decoded.enabled);
}

#[test]
fn enabled_defaults_to_true_when_absent() {
    let toml_str = r#"
id = "src_x"
kind = "conversation"
label = "Convos"
"#;
    let decoded: MemorySourceEntry = toml::from_str(toml_str).unwrap();
    assert!(decoded.enabled);
}

/// A fully-`None` entry used as a `..default_entry()` base in the tests above.
pub(super) fn default_entry() -> MemorySourceEntry {
    MemorySourceEntry {
        id: String::new(),
        kind: SourceKind::Folder,
        label: String::new(),
        enabled: true,
        toolkit: None,
        connection_id: None,
        path: None,
        glob: None,
        url: None,
        branch: None,
        paths: Vec::new(),
        max_commits: None,
        max_issues: None,
        max_prs: None,
        max_items: None,
        selector: None,
        max_tokens_per_sync: None,
        max_cost_per_sync_usd: None,
        sync_depth_days: None,
    }
}

#[test]
fn max_items_is_applicable_to_composio_and_rss_but_not_other_kinds() {
    // The host UI exposes `max_items` for Composio sources and creates them with
    // a toolkit default, so editing one must not be rejected — the regression
    // this guards ("field 'max_items' is not applicable to source kind
    // 'composio'"). RSS keeps it; kinds with no per-run item cap still reject.
    let patch = || MemorySourcePatch {
        max_items: Some(Some(100)),
        ..Default::default()
    };
    assert!(patch().validate_for_kind(SourceKind::Composio).is_ok());
    assert!(patch().validate_for_kind(SourceKind::RssFeed).is_ok());
    assert!(patch().validate_for_kind(SourceKind::Folder).is_err());
    assert!(patch().validate_for_kind(SourceKind::GithubRepo).is_err());
    assert!(patch().validate_for_kind(SourceKind::WebPage).is_err());
}

/// The engine keeps its own copy of these types in `memory/sources/types.rs`,
/// and the two are joined by a live wire: `tinymemory-core`'s engine seam
/// converts between them with `serde_json::to_value` / `from_value` for the
/// tree-coupled source kinds, in both directions. Nothing but the serialised
/// shape holds that seam together — the copies are distinct Rust types in
/// distinct crates and neither compiles against the other.
///
/// So a renamed field or a new `SourceKind` variant on either side is not a
/// compile error. It is a runtime failure on the first external-source sync
/// after the engine pin moves, at the point of conversion, far from the edit
/// that caused it.
///
/// These pin the full serialised shape of each type that crosses. A failure
/// here means the copies have diverged and the change needs coordinating
/// across both crates, never a local edit to the expectation.
#[test]
fn source_entry_wire_format_is_pinned() {
    let entry = MemorySourceEntry {
        id: "src_pinned".into(),
        kind: SourceKind::GithubRepo,
        label: "Pinned".into(),
        enabled: false,
        toolkit: Some("gmail".into()),
        connection_id: Some("conn-1".into()),
        path: Some("/notes".into()),
        glob: Some("**/*.md".into()),
        url: Some("https://github.com/tinyhumansai/tinymemory".into()),
        branch: Some("main".into()),
        paths: vec!["core/src".into()],
        max_commits: Some(10),
        max_issues: Some(20),
        max_prs: Some(30),
        query: Some("from:me".into()),
        max_items: Some(40),
        selector: Some("article".into()),
        max_tokens_per_sync: Some(50_000),
        max_cost_per_sync_usd: Some(1.5),
        sync_depth_days: Some(90),
    };

    assert_eq!(
        serde_json::to_value(&entry).unwrap(),
        serde_json::json!({
            "id": "src_pinned",
            "kind": "github_repo",
            "label": "Pinned",
            "enabled": false,
            "toolkit": "gmail",
            "connection_id": "conn-1",
            "path": "/notes",
            "glob": "**/*.md",
            "url": "https://github.com/tinyhumansai/tinymemory",
            "branch": "main",
            "paths": ["core/src"],
            "max_commits": 10,
            "max_issues": 20,
            "max_prs": 30,
            "max_items": 40,
            "selector": "article",
            "max_tokens_per_sync": 50000,
            "max_cost_per_sync_usd": 1.5,
            "sync_depth_days": 90
        })
    );
}

/// Every optional field is skipped when absent, so an entry carrying only its
/// required fields is a four-key object. A `skip_serializing_if` dropped from
/// one copy and not the other changes what the seam sends without changing
/// what either side compiles.
#[test]
fn an_empty_source_entry_serialises_to_its_required_fields_only() {
    let entry = MemorySourceEntry {
        id: "src_min".into(),
        label: "Minimal".into(),
        ..default_entry()
    };

    assert_eq!(
        serde_json::to_value(&entry).unwrap(),
        serde_json::json!({
            "id": "src_min",
            "kind": "folder",
            "label": "Minimal",
            "enabled": true
        })
    );
}

/// `SourceItem` crosses the same seam, on the `list_items` direction.
#[test]
fn source_item_wire_format_is_pinned() {
    assert_eq!(
        serde_json::to_value(SourceItem {
            id: "item-1".into(),
            title: "Quarterly planning".into(),
            updated_at_ms: Some(1_777_000_000_000),
        })
        .unwrap(),
        serde_json::json!({
            "id": "item-1",
            "title": "Quarterly planning",
            "updated_at_ms": 1_777_000_000_000i64
        })
    );

    assert_eq!(
        serde_json::to_value(SourceItem {
            id: "item-2".into(),
            title: "No timestamp".into(),
            updated_at_ms: None,
        })
        .unwrap(),
        serde_json::json!({ "id": "item-2", "title": "No timestamp" })
    );
}

/// `SourceContent` crosses the same seam, on the `read_item` direction.
#[test]
fn source_content_wire_format_is_pinned() {
    assert_eq!(
        serde_json::to_value(SourceContent {
            id: "item-1".into(),
            title: "Quarterly planning".into(),
            body: "# Roadmap".into(),
            content_type: ContentType::Markdown,
            metadata: serde_json::json!({ "author": "shanu" }),
        })
        .unwrap(),
        serde_json::json!({
            "id": "item-1",
            "title": "Quarterly planning",
            "body": "# Roadmap",
            "content_type": "markdown",
            "metadata": { "author": "shanu" }
        })
    );
}
