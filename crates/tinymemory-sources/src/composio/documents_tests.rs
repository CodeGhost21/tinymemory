//! Tests for turning Composio payloads into documents and `StoreItem`s.

use super::*;
use serde_json::json;
use tinymemory_api::ItemKind;

fn text_of(item: &StoreItem) -> (&Option<String>, &str, &MemoryMeta) {
    match item {
        StoreItem::Document {
            title,
            body: DocumentBody::Text(text),
            meta,
            ..
        } => (title, text.as_str(), meta),
        other => panic!("expected a text document, got {other:?}"),
    }
}

#[test]
fn a_github_search_becomes_items_with_url_repo_time_and_toolkit_tag() {
    let data = json!({
        "data": { "items": [{
            "id": 42,
            "title": "Fix the build",
            "body": "The build is **red**.",
            "html_url": "https://github.com/acme/widgets/issues/7",
            "updated_at": "2024-05-21T15:30:00Z"
        }]}
    });
    let items = payload_items("github", "conn_gh", &data);
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.kind(), ItemKind::Document);
    item.validate().unwrap();

    let (title, body, meta) = text_of(item);
    assert_eq!(title.as_deref(), Some("GitHub: acme/widgets#7: Fix the build"));
    assert_eq!(body, "The build is **red**.");
    assert_eq!(meta.source.kind, SourceKind::Composio);
    assert_eq!(meta.source.id.as_deref(), Some("conn_gh"));
    assert_eq!(meta.tags, vec!["github".to_string()]);
    assert_eq!(
        meta.url.as_deref(),
        Some("https://github.com/acme/widgets/issues/7")
    );
    assert_eq!(meta.repo.as_deref(), Some("acme/widgets"));
    assert_eq!(
        meta.observed_at,
        Some(Utc.with_ymd_and_hms(2024, 5, 21, 15, 30, 0).unwrap())
    );
}

#[test]
fn post_processed_gmail_messages_carry_headers_thread_and_date() {
    let data = json!({ "messages": [{
        "id": "m1",
        "threadId": "t1",
        "subject": "Lunch?",
        "from": "Ann <ann@example.com>",
        "to": "me@example.com",
        "date": "Tue, 21 May 2024 12:00:00 +0000",
        "markdown": "Tacos at noon."
    }]});
    let documents = normalise_payload("gmail", &data);
    assert_eq!(documents.len(), 1);
    let document = &documents[0];
    assert_eq!(document.title.as_deref(), Some("Lunch?"));
    assert!(document.body.starts_with("From: Ann <ann@example.com>\n"));
    assert!(document.body.ends_with("Tacos at noon."));
    assert_eq!(document.thread_id.as_deref(), Some("t1"));
    assert_eq!(document.id.as_deref(), Some("m1"));
    assert_eq!(
        document.observed_at,
        Some(Utc.with_ymd_and_hms(2024, 5, 21, 12, 0, 0).unwrap())
    );
}

#[test]
fn slack_messages_take_permalink_and_ts_time() {
    let data = json!({ "messages": [{
        "ts": "1716300000.000100",
        "user": "U1",
        "channel_id": "C1",
        "text": "deploy done",
        "permalink": "https://acme.slack.com/archives/C1/p1716300000000100"
    }]});
    let items = payload_items("slack", "src_slack", &data);
    let (title, body, meta) = text_of(&items[0]);
    assert_eq!(title.as_deref(), Some("Slack message from U1 in C1"));
    assert_eq!(body, "deploy done");
    assert_eq!(
        meta.url.as_deref(),
        Some("https://acme.slack.com/archives/C1/p1716300000000100")
    );
    assert_eq!(
        meta.observed_at.map(|at| at.timestamp()),
        Some(1_716_300_000)
    );
    assert_eq!(meta.tags, vec!["slack".to_string()]);
}

#[test]
fn linear_notion_and_clickup_records_are_normalised() {
    let linear = json!({ "nodes": [{
        "identifier": "ENG-1",
        "title": "Ship v2",
        "description": "All of it.",
        "url": "https://linear.app/acme/issue/ENG-1",
        "updatedAt": "2024-01-02T03:04:05Z"
    }]});
    let documents = normalise_payload("linear", &linear);
    assert_eq!(documents[0].id.as_deref(), Some("ENG-1"));
    assert_eq!(documents[0].body, "All of it.");
    assert!(documents[0].observed_at.is_some());

    let notion = json!({ "results": [{
        "id": "p1",
        "url": "https://notion.so/p1",
        "last_edited_time": "2024-01-02T03:04:05.000Z",
        "properties": { "Name": { "type": "title", "title": [{ "plain_text": "Roadmap" }] } }
    }]});
    let documents = normalise_payload("notion", &notion);
    assert_eq!(documents[0].title.as_deref(), Some("Roadmap"));
    assert_eq!(documents[0].body, "Roadmap", "a page without body keeps its title");
    assert_eq!(documents[0].url.as_deref(), Some("https://notion.so/p1"));

    let page_markdown = json!({ "data": { "markdown": "# Plan\n\nDo it." }, "id": "p2" });
    let documents = normalise_payload("notion", &page_markdown);
    assert_eq!(documents.len(), 1);
    assert_eq!(documents[0].body, "# Plan\n\nDo it.");

    let clickup = json!({ "tasks": [{
        "id": "t9",
        "name": "Write docs",
        "description": "<p>The <b>README</b></p>",
        "url": "https://app.clickup.com/t/t9",
        "date_updated": "1700000000000"
    }]});
    let documents = normalise_payload("clickup", &clickup);
    assert_eq!(documents[0].id.as_deref(), Some("t9"));
    assert_eq!(
        documents[0].observed_at.map(|at| at.timestamp()),
        Some(1_700_000_000)
    );
}

#[test]
fn html_bodies_are_converted_to_markdown() {
    let data = json!({ "nodes": [{
        "title": "Page",
        "description": "<!DOCTYPE html><html><body><h2>Hi</h2><p>There</p></body></html>"
    }]});
    let documents = normalise_payload("linear", &data);
    assert_eq!(documents[0].body, "## Hi\n\nThere");
}

#[test]
fn records_without_any_text_are_skipped() {
    let data = json!({ "messages": [{ "ts": "1.0", "text": "  " }] });
    assert!(normalise_payload("slack", &data).is_empty());
    assert!(normalise_payload("github", &json!({ "items": [{}] })).is_empty());
}

#[test]
fn an_unknown_toolkit_keeps_each_record_as_fenced_json() {
    let data = json!({ "data": { "items": [
        { "id": 1, "name": "Alpha", "url": "https://example.com/1", "updated_at": "2024-01-01T00:00:00Z" },
        { "id": 2 }
    ]}});
    let items = payload_items("hubspot", "conn_hs", &data);
    assert_eq!(items.len(), 2);
    let (title, body, meta) = text_of(&items[0]);
    assert_eq!(title.as_deref(), Some("Alpha"));
    assert!(body.starts_with("```json\n"));
    assert!(body.contains("\"name\": \"Alpha\""));
    assert_eq!(meta.url.as_deref(), Some("https://example.com/1"));
    assert_eq!(meta.tags, vec!["hubspot".to_string()]);

    let single = normalise_payload("hubspot", &json!({ "ok": true }));
    assert_eq!(single.len(), 1);
}

#[test]
fn slack_timestamps_parse_with_and_without_a_fraction() {
    assert_eq!(parse_slack_ts("10").map(|at| at.timestamp()), Some(10));
    assert_eq!(
        parse_slack_ts("10.5").map(|at| at.timestamp_subsec_micros()),
        Some(500_000)
    );
    assert_eq!(parse_slack_ts("x"), None);
}
