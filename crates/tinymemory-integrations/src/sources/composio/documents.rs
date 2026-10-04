//! One Composio response in, documents and `StoreItem`s out.
//!
//! [`normalise_payload`] dispatches on the toolkit slug to the matching
//! normaliser and reads each record's title, body, link and timestamp.
//! Toolkits without a dedicated normaliser fall back to a generic walk that
//! keeps each record as fenced JSON, so a new toolkit is ingested (verbosely)
//! rather than dropped. [`payload_items`] wraps the documents as
//! `StoreItem::Document`s.

use crate::documents::{DocumentFormat, markdown_from_text};
use chrono::{DateTime, TimeZone, Utc};
use serde_json::Value;
use tinymemory_api::{DocumentBody, MemoryMeta, SourceKind, StoreItem};

use super::fields::pick_str;
use super::{clickup, github, gmail_post_process, linear, notion};

/// One record of a Composio payload, normalised: an email, a message, an
/// issue, a task or a page.
#[derive(Debug, Clone, PartialEq)]
pub struct ComposioDocument {
    /// The provider's id for the record, when it has one.
    pub id: Option<String>,
    /// A human-readable title.
    pub title: Option<String>,
    /// The record's text as markdown. Never empty.
    pub body: String,
    /// A link back to the record in the provider's UI.
    pub url: Option<String>,
    /// When the record was last updated or sent.
    pub observed_at: Option<DateTime<Utc>>,
    /// The email or message thread the record belongs to.
    pub thread_id: Option<String>,
    /// The repository a GitHub record belongs to, as `owner/name`.
    pub repo: Option<String>,
}

impl ComposioDocument {
    /// A record with only a body.
    fn with_body(body: String) -> Self {
        Self {
            id: None,
            title: None,
            body,
            url: None,
            observed_at: None,
            thread_id: None,
            repo: None,
        }
    }

    /// Wrap this record as a [`StoreItem::Document`] from `toolkit`, read
    /// through the connection or source `source_id`.
    #[must_use]
    pub fn into_store_item(self, toolkit: &str, source_id: &str) -> StoreItem {
        let mut meta = MemoryMeta::from_source(SourceKind::Composio, Some(source_id.to_string()));
        meta.url = self.url;
        meta.observed_at = self.observed_at;
        meta.thread_id = self.thread_id;
        meta.repo = self.repo;
        meta.tags = vec![toolkit.to_string()];
        StoreItem::Document {
            title: self.title,
            body: DocumentBody::Text(self.body),
            mime: Some(DocumentFormat::Markdown.mime().to_string()),
            meta,
        }
    }
}

/// Normalise one Composio response from `toolkit` into documents.
///
/// `data` is the action's response; for Gmail and Slack, run
/// [`gmail_post_process::post_process`] / [`super::slack_post_process::post_process`]
/// on it first so it carries the slim `messages[]` shape. Records with no text
/// at all are skipped.
#[must_use]
pub fn normalise_payload(toolkit: &str, data: &Value) -> Vec<ComposioDocument> {
    let documents: Vec<ComposioDocument> = match toolkit.to_ascii_lowercase().as_str() {
        "gmail" => array_at(data, &["/messages", "/data/messages"])
            .iter()
            .filter_map(gmail_message)
            .collect(),
        "slack" => array_at(data, &["/messages", "/data/messages"])
            .iter()
            .filter_map(slack_message)
            .collect(),
        "github" => github::extract_issues(data)
            .iter()
            .filter_map(github_issue)
            .collect(),
        "linear" => linear::extract_issues(data)
            .iter()
            .filter_map(linear_issue)
            .collect(),
        "notion" => notion_pages(data),
        "clickup" => clickup::extract_tasks(data)
            .iter()
            .filter_map(clickup_task)
            .collect(),
        _ => generic_records(data),
    };
    log::debug!(
        "[memory_sources:composio] normalised toolkit={toolkit} documents={}",
        documents.len()
    );
    documents
}

/// Normalise one Composio response and wrap every record as a
/// [`StoreItem::Document`] with `source.kind = Composio`,
/// `source.id = source_id` and `tags = [toolkit]`.
#[must_use]
pub fn payload_items(toolkit: &str, source_id: &str, data: &Value) -> Vec<StoreItem> {
    normalise_payload(toolkit, data)
        .into_iter()
        .map(|document| document.into_store_item(toolkit, source_id))
        .collect()
}

/// The first array found at any of `pointers`.
fn array_at<'a>(data: &'a Value, pointers: &[&str]) -> &'a [Value] {
    pointers
        .iter()
        .find_map(|pointer| data.pointer(pointer).and_then(Value::as_array))
        .map_or(&[], Vec::as_slice)
}

/// A body as markdown: HTML (as sniffed) is converted, anything else kept.
fn to_markdown(text: &str) -> String {
    let format = DocumentFormat::sniff(text.as_bytes(), None, None);
    markdown_from_text(text, format).trim().to_string()
}

/// Build a document from its parts, falling back to the title as the body;
/// `None` when there is no text at all.
fn document(title: Option<String>, body: Option<String>) -> Option<ComposioDocument> {
    let body = body
        .map(|body| to_markdown(&body))
        .filter(|body| !body.is_empty())
        .or_else(|| title.clone())?;
    let mut document = ComposioDocument::with_body(body);
    document.title = title;
    Some(document)
}

/// Parse an ISO 8601 / RFC 3339 / RFC 2822 timestamp.
fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    gmail_post_process::parse_email_date(text)
}

/// Parse an epoch-milliseconds string (ClickUp's `date_updated`).
fn parse_epoch_ms(text: &str) -> Option<DateTime<Utc>> {
    let millis = text.trim().parse::<i64>().ok()?;
    Utc.timestamp_millis_opt(millis).single()
}

/// Parse a Slack `ts` (`"1712345678.123456"`, epoch seconds with a fraction).
fn parse_slack_ts(text: &str) -> Option<DateTime<Utc>> {
    let (seconds, fraction) = text.split_once('.').unwrap_or((text, "0"));
    let seconds = seconds.parse::<i64>().ok()?;
    let micros = format!("{fraction:0<6}").get(..6)?.parse::<u32>().ok()?;
    Utc.timestamp_opt(seconds, micros * 1_000).single()
}

/// A string field, or a number rendered as a string.
fn scalar(value: &Value, key: &str) -> Option<String> {
    match value.get(key)? {
        Value::String(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

/// A post-processed Gmail message: headers above the body.
fn gmail_message(message: &Value) -> Option<ComposioDocument> {
    let subject = pick_str(message, &["subject"]);
    let markdown = pick_str(message, &["markdown", "messageText"]);
    let mut header = String::new();
    for (label, key) in [("From", "from"), ("To", "to"), ("Date", "date")] {
        if let Some(value) = pick_str(message, &[key]) {
            header.push_str(&format!("{label}: {value}\n"));
        }
    }
    let body = match (markdown, header.is_empty()) {
        (Some(markdown), false) => Some(format!("{header}\n{markdown}")),
        (Some(markdown), true) => Some(markdown),
        (None, _) => None,
    };
    let mut document = document(subject, body)?;
    document.id = pick_str(message, &["id", "messageId"]);
    document.thread_id = pick_str(message, &["threadId", "thread_id"]);
    document.observed_at = pick_str(message, &["date"]).as_deref().and_then(parse_time);
    Some(document)
}

/// A post-processed Slack message.
fn slack_message(message: &Value) -> Option<ComposioDocument> {
    let user = pick_str(message, &["user"]);
    let channel = pick_str(message, &["channel_id"]);
    let title = match (&user, &channel) {
        (Some(user), Some(channel)) => Some(format!("Slack message from {user} in {channel}")),
        (Some(user), None) => Some(format!("Slack message from {user}")),
        (None, _) => Some("Slack message".to_string()),
    };
    let text = pick_str(message, &["text"])?;
    let mut document = document(title, Some(text))?;
    let ts = pick_str(message, &["ts"]);
    document.id = ts.clone();
    document.url = pick_str(message, &["permalink"]);
    document.thread_id = pick_str(message, &["thread_ts"]);
    document.observed_at = ts.as_deref().and_then(parse_slack_ts);
    Some(document)
}

/// A GitHub issue or pull request from a search response.
fn github_issue(issue: &Value) -> Option<ComposioDocument> {
    let mut document = document(
        github::extract_issue_title(issue),
        pick_str(issue, &["body", "data.body"]),
    )?;
    document.id = github::extract_issue_id(issue);
    document.url = pick_str(issue, &["html_url", "data.html_url"]);
    document.repo = document.url.as_deref().and_then(github_repo);
    document.observed_at = github::extract_issue_updated_at(issue)
        .as_deref()
        .and_then(parse_time);
    Some(document)
}

/// `owner/name` from a `https://github.com/owner/name/...` link.
fn github_repo(url: &str) -> Option<String> {
    let rest = url.split_once("github.com/")?.1;
    let mut parts = rest.split('/');
    let owner = parts.next().filter(|part| !part.is_empty())?;
    let name = parts.next().filter(|part| !part.is_empty())?;
    Some(format!("{owner}/{name}"))
}

/// A Linear issue.
fn linear_issue(issue: &Value) -> Option<ComposioDocument> {
    let mut document = document(
        linear::extract_issue_title(issue),
        pick_str(issue, &["description", "data.description"]),
    )?;
    document.id = pick_str(issue, &["identifier", "id", "data.identifier", "data.id"]);
    document.url = pick_str(issue, &["url", "data.url"]);
    document.observed_at = linear::extract_issue_updated(issue)
        .as_deref()
        .and_then(parse_time);
    Some(document)
}

/// Notion pages from a search response, or the one page a
/// `NOTION_GET_PAGE_MARKDOWN` response carries.
fn notion_pages(data: &Value) -> Vec<ComposioDocument> {
    let results = notion::extract_results(data);
    if results.is_empty() {
        return notion::extract_page_markdown(data)
            .and_then(|markdown| {
                let title = notion::extract_page_title(data);
                let mut document = document(title, Some(markdown))?;
                document.id = pick_str(data, &["id", "data.id", "page_id", "data.page_id"]);
                document.url = pick_str(data, &["url", "data.url"]);
                Some(document)
            })
            .into_iter()
            .collect();
    }
    results
        .iter()
        .filter_map(|page| {
            let mut document = document(
                notion::extract_page_title(page),
                notion::extract_page_markdown(page),
            )?;
            document.id = pick_str(page, &["id", "data.id"]);
            document.url = pick_str(page, &["url", "data.url"]);
            document.observed_at = pick_str(page, &["last_edited_time", "data.last_edited_time"])
                .as_deref()
                .and_then(parse_time);
            Some(document)
        })
        .collect()
}

/// A ClickUp task.
fn clickup_task(task: &Value) -> Option<ComposioDocument> {
    let mut document = document(
        clickup::extract_task_name(task),
        pick_str(
            task,
            &["markdown_description", "description", "text_content"],
        ),
    )?;
    document.id = scalar(task, "id");
    document.url = pick_str(task, &["url", "data.url"]);
    document.observed_at = clickup::extract_task_updated(task)
        .as_deref()
        .and_then(|text| parse_epoch_ms(text).or_else(|| parse_time(text)));
    Some(document)
}

/// Any other toolkit: each record in the first list found (or the whole
/// payload) as fenced JSON, titled and linked when it says how.
fn generic_records(data: &Value) -> Vec<ComposioDocument> {
    let records = array_at(
        data,
        &[
            "/data/items",
            "/items",
            "/data/results",
            "/results",
            "/data/data",
            "/data",
        ],
    );
    let records: Vec<&Value> = if records.is_empty() {
        vec![data]
    } else {
        records.iter().collect()
    };
    records
        .into_iter()
        .filter(|record| !record.is_null())
        .filter_map(|record| {
            let json = serde_json::to_string_pretty(record).ok()?;
            let title = pick_str(record, &["title", "name", "subject"]);
            let mut document = ComposioDocument::with_body(format!("```json\n{json}\n```"));
            document.title = title;
            document.id = scalar(record, "id");
            document.url = pick_str(record, &["url", "html_url", "permalink", "link"]);
            document.observed_at = pick_str(record, &["updated_at", "updatedAt", "created_at"])
                .as_deref()
                .and_then(parse_time);
            Some(document)
        })
        .collect()
}

#[cfg(test)]
#[path = "documents_tests.rs"]
mod tests;
