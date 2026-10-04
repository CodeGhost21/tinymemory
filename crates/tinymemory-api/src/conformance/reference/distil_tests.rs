//! The reference engine's toy belief distillation.

use super::*;
use crate::{MemoryMeta, Namespace, Reach, Turn};

fn at(namespace: Namespace) -> MemoryMeta {
    MemoryMeta {
        namespace,
        ..MemoryMeta::default()
    }
}

#[test]
fn distils_the_first_sentence_of_documents_and_user_turns() {
    let items = vec![
        StoreItem::document(
            "# Refunds\n\nRefunds take five days. Ask support.",
            at(Namespace::source("markdown")),
        ),
        StoreItem::Conversation {
            turns: vec![
                Turn::new(Role::Assistant, "Hello!"),
                Turn::new(Role::User, "I live in Lagos. What is the weather?"),
            ],
            meta: at(Namespace::agent("support")),
        },
        StoreItem::learning("already a belief", LearningKind::Fact, 0.9, at(Namespace::ROOT)),
    ];
    let beliefs = distil(&items, &ConsolidateRequest::new(Reach::subtree(Namespace::ROOT)));
    let texts: Vec<String> = beliefs.iter().map(StoreItem::render_text).collect();
    assert_eq!(texts, ["Refunds", "I live in Lagos."]);
    let StoreItem::Learning { evidence, meta, .. } = &beliefs[1] else {
        panic!("a belief is a learning");
    };
    assert_eq!(evidence.as_deref(), Some(items[1].fingerprint().as_str()));
    assert_eq!(meta.namespace, Namespace::agent("support"));
    assert_eq!(meta.tags, [CONSOLIDATED_TAG]);
}

#[test]
fn honours_the_reach_and_the_kinds() {
    let items = vec![
        StoreItem::document("Pdf fact.", at(Namespace::source("pdf"))),
        StoreItem::document("Notion fact.", at(Namespace::source("notion"))),
    ];
    let pdf_only = ConsolidateRequest::new(Reach::exact(Namespace::source("pdf")));
    assert_eq!(distil(&items, &pdf_only).len(), 1);
    let conversations_only =
        ConsolidateRequest::new(Reach::subtree(Namespace::ROOT)).kinds([ItemKind::Conversation]);
    assert!(distil(&items, &conversations_only).is_empty());
}

#[test]
fn skips_blank_text_and_caps_long_sentences() {
    assert_eq!(first_sentence("  \n#  \n"), None);
    let long = "word ".repeat(200);
    assert_eq!(
        first_sentence(&long).map(|s| s.chars().count()),
        Some(MAX_STATEMENT_CHARS)
    );
}
