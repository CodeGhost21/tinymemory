//! Budgeting, trimming order and frontmatter.

use chrono::TimeZone;

use super::*;

fn at() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 10, 2, 12, 0, 0).unwrap()
}

fn brief(heading: &str, body: &str, refs: &[&str]) -> BriefSection {
    BriefSection {
        heading: heading.to_string(),
        body: body.to_string(),
        refs: refs.iter().map(|id| ItemId::from(*id)).collect(),
    }
}

fn learning(id: &str, text: &str) -> LearningLine {
    LearningLine {
        id: ItemId::from(id),
        text: text.to_string(),
    }
}

fn sections() -> Sections {
    Sections {
        briefs: vec![
            brief("About the user", &"Steven builds memory systems. ".repeat(4), &["a", "b"]),
            brief("Active work", &"Working on memory v2 in Rust. ".repeat(4), &["b", "c"]),
        ],
        learnings: (0..10)
            .map(|i| learning(&format!("l{i}"), &format!("learning number {i}\nwrapped")))
            .collect(),
    }
}

#[test]
fn tokens_are_four_characters_rounded_up() {
    assert_eq!(estimate_tokens(""), 0);
    assert_eq!(estimate_tokens("abcd"), 1);
    assert_eq!(estimate_tokens("abcde"), 2);
    assert_eq!(estimate_tokens("ééééé"), 2);
}

#[test]
fn an_ample_budget_keeps_everything_with_frontmatter() {
    let rendered = render(sections(), 10_000, "reference", at());
    let md = &rendered.markdown;
    assert!(md.starts_with("---\ngenerated_at: 2026-10-02T12:00:00Z\nengine: reference\n"));
    assert!(md.contains(&format!("tokens: {}\n", rendered.tokens)));
    assert!(md.contains("refs: [a, b, c, l0, l1"));
    let about = md.find("## About the user").unwrap();
    let active = md.find("## Active work").unwrap();
    let learnings = md.find("## Learnings").unwrap();
    assert!(about < active && active < learnings);
    assert!(md.contains("- learning number 0 wrapped\n"));
    assert_eq!(rendered.tokens, estimate_tokens(md));
    assert_eq!(rendered.refs.len(), 13);
}

#[test]
fn learnings_are_trimmed_before_any_brief() {
    let full = render(sections(), 10_000, "reference", at());
    let budget = full.tokens - 20;
    let rendered = render(sections(), budget, "reference", at());
    assert!(rendered.tokens <= budget);
    assert!(rendered.markdown.contains(&"Steven builds memory systems. ".repeat(4).trim().to_string()));
    assert!(rendered.markdown.contains(&"Working on memory v2 in Rust. ".repeat(4).trim().to_string()));
    assert!(!rendered.markdown.contains("learning number 9"));
    assert!(rendered.markdown.contains("learning number 0"));
    assert!(!rendered.refs.contains(&ItemId::from("l9")));
}

#[test]
fn once_learnings_are_gone_the_last_brief_shrinks_then_drops() {
    let without_learnings = Sections {
        learnings: Vec::new(),
        ..sections()
    };
    let full = render(without_learnings.clone(), 10_000, "reference", at());
    let shrunk = render(without_learnings.clone(), full.tokens - 10, "reference", at());
    assert!(shrunk.tokens <= full.tokens - 10);
    assert!(shrunk.markdown.contains("## Active work"));
    assert!(shrunk.markdown.contains('…'));
    assert!(shrunk.markdown.contains(&"Steven builds memory systems. ".repeat(4).trim().to_string()));

    let tight = render(without_learnings, full.tokens - 45, "reference", at());
    assert!(tight.tokens <= full.tokens - 45);
    assert!(tight.markdown.contains("## About the user"));
    assert!(!tight.markdown.contains("## Active work"));
    assert!(!tight.refs.contains(&ItemId::from("c")));
}

#[test]
fn nothing_to_say_or_no_room_is_an_empty_document() {
    let empty = Sections {
        briefs: Vec::new(),
        learnings: Vec::new(),
    };
    let rendered = render(empty, 100, "reference", at());
    assert_eq!(rendered.markdown, "");
    assert_eq!(rendered.tokens, 0);
    let squeezed = render(sections(), 5, "reference", at());
    assert_eq!(squeezed.markdown, "");
    assert!(squeezed.refs.is_empty());
}
