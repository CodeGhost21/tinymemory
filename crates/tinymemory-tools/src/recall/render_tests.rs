//! Budgeting, trimming order and frontmatter, through the `context.md`
//! shape (prose briefs, then a learnings list) and through plain packs.

use chrono::TimeZone;

use super::*;

/// One answered brief, as `context.md` has them.
#[derive(Debug, Clone)]
struct BriefSection {
    heading: String,
    body: String,
    refs: Vec<ItemId>,
}

/// One learning line.
type LearningLine = Line;

/// The `context.md` shape: prose briefs, then one learnings list.
#[derive(Debug, Clone)]
struct Sections {
    briefs: Vec<BriefSection>,
    learnings: Vec<LearningLine>,
}

/// Renders the `context.md` shape with frontmatter, as the compiler does.
fn render_doc(
    sections: Sections,
    budget_tokens: usize,
    engine: &str,
    generated_at: DateTime<Utc>,
) -> Rendered {
    let mut all: Vec<Section> = sections
        .briefs
        .into_iter()
        .map(|brief| Section {
            heading: brief.heading,
            body: Body::Prose {
                text: brief.body,
                refs: brief.refs,
            },
        })
        .collect();
    all.push(Section {
        heading: "Learnings".to_string(),
        body: Body::Lines(sections.learnings),
    });
    render(
        all,
        budget_tokens,
        "Context",
        Some(Frontmatter {
            engine,
            generated_at,
        }),
    )
}

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
            brief(
                "About the user",
                &"Steven builds memory systems. ".repeat(4),
                &["a", "b"],
            ),
            brief(
                "Active work",
                &"Working on memory v2 in Rust. ".repeat(4),
                &["b", "c"],
            ),
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
    let rendered = render_doc(sections(), 10_000, "reference", at());
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
    let full = render_doc(sections(), 10_000, "reference", at());
    let budget = full.tokens - 20;
    let rendered = render_doc(sections(), budget, "reference", at());
    assert!(rendered.tokens <= budget);
    assert!(
        rendered.markdown.contains(
            &"Steven builds memory systems. "
                .repeat(4)
                .trim()
                .to_string()
        )
    );
    assert!(
        rendered.markdown.contains(
            &"Working on memory v2 in Rust. "
                .repeat(4)
                .trim()
                .to_string()
        )
    );
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
    let full = render_doc(without_learnings.clone(), 10_000, "reference", at());
    let shrunk = render_doc(
        without_learnings.clone(),
        full.tokens - 10,
        "reference",
        at(),
    );
    assert!(shrunk.tokens <= full.tokens - 10);
    assert!(shrunk.markdown.contains("## Active work"));
    assert!(shrunk.markdown.contains('…'));
    assert!(
        shrunk.markdown.contains(
            &"Steven builds memory systems. "
                .repeat(4)
                .trim()
                .to_string()
        )
    );

    let tight = render_doc(without_learnings, full.tokens - 45, "reference", at());
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
    let rendered = render_doc(empty, 100, "reference", at());
    assert_eq!(rendered.markdown, "");
    assert_eq!(rendered.tokens, 0);
    let squeezed = render_doc(sections(), 5, "reference", at());
    assert_eq!(squeezed.markdown, "");
    assert!(squeezed.refs.is_empty());
}

fn lines(heading: &str, ids: &[&str]) -> Section {
    Section {
        heading: heading.to_string(),
        body: Body::Lines(
            ids.iter()
                .map(|id| Line {
                    id: ItemId::from(*id),
                    text: format!("hit {id} with some words in it"),
                })
                .collect(),
        ),
    }
}

#[test]
fn a_pack_without_frontmatter_is_just_the_sections() {
    let rendered = render(
        vec![lines("Brain", &["b1"]), lines("Learnings", &["l1"])],
        10_000,
        "Memory",
        None,
    );
    assert_eq!(
        rendered.markdown,
        "# Memory\n\n## Brain\n\n- hit b1 with some words in it\n\n## Learnings\n\n- hit l1 with some words in it\n"
    );
    assert_eq!(rendered.tokens, estimate_tokens(&rendered.markdown));
    assert_eq!(rendered.refs, [ItemId::from("b1"), ItemId::from("l1")]);
}

#[test]
fn the_last_lines_section_is_trimmed_first_and_dropped_when_empty() {
    let all = vec![lines("First", &["a1", "a2"]), lines("Last", &["z1", "z2"])];
    let full = render(all.clone(), 10_000, "Memory", None);
    let rendered = render(all.clone(), full.tokens - 5, "Memory", None);
    assert!(rendered.markdown.contains("z1") && !rendered.markdown.contains("z2"));
    assert!(rendered.markdown.contains("a2"));
    let tighter = render(all, full.tokens - 25, "Memory", None);
    assert!(
        !tighter.markdown.contains("## Last"),
        "{}",
        tighter.markdown
    );
    assert!(tighter.markdown.contains("## First"));
}

#[test]
fn empty_sections_never_render_a_heading() {
    let rendered = render(
        vec![
            lines("Nothing", &[]),
            Section {
                heading: "Blank".to_string(),
                body: Body::Prose {
                    text: "  ".to_string(),
                    refs: Vec::new(),
                },
            },
        ],
        10_000,
        "Memory",
        None,
    );
    assert_eq!(rendered.markdown, "");
}
