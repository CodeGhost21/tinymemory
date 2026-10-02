//! Tests for building a `StoreItem::Document` from a raw document.

use super::*;

use tinymemory_api::{ItemKind, SourceKind};

use crate::convert::ConverterChain;
use crate::error::Error;

fn parts(item: StoreItem) -> (Option<String>, String, Option<String>, MemoryMeta) {
    match item {
        StoreItem::Document {
            title,
            body: DocumentBody::Text(text),
            mime,
            meta,
        } => (title, text, mime, meta),
        other => panic!("expected a text document, got {other:?}"),
    }
}

#[tokio::test]
async fn html_becomes_a_markdown_document_with_its_title_and_mime() {
    let chain = ConverterChain::default();
    let document = RawDocument::new(
        "<html><head><title>Notes</title></head><body><h1>Hi</h1><p>Body.</p></body></html>",
    )
    .with_mime("text/html");
    let meta = MemoryMeta::from_source(SourceKind::Link, Some("src_1".into()));
    let item = document_item(&chain, &document, meta).await.unwrap();
    assert_eq!(item.kind(), ItemKind::Document);
    item.validate().unwrap();

    let (title, body, mime, meta) = parts(item);
    assert_eq!(title.as_deref(), Some("Notes"));
    assert_eq!(body, "# Hi\n\nBody.");
    assert_eq!(mime.as_deref(), Some("text/html"));
    assert_eq!(meta.source.kind, SourceKind::Link);
    assert_eq!(meta.source.id.as_deref(), Some("src_1"));
    assert_eq!(meta.language, None);
}

#[tokio::test]
async fn code_keeps_its_text_and_gains_its_language() {
    let chain = ConverterChain::default();
    let source = "#!/usr/bin/env python\n# a comment\nprint('hi')\n";
    let document = RawDocument::new(source).with_filename("scripts/hello.py");
    let item = document_item(&chain, &document, MemoryMeta::default())
        .await
        .unwrap();

    let (title, body, mime, meta) = parts(item);
    assert_eq!(body, source);
    assert_eq!(title.as_deref(), Some("hello.py"), "a comment is not a title");
    assert_eq!(mime.as_deref(), Some("text/x-source"));
    assert_eq!(meta.language.as_deref(), Some("python"));
}

#[tokio::test]
async fn a_caller_supplied_language_is_never_overwritten() {
    let chain = ConverterChain::default();
    let document = RawDocument::new("fn main() {}").with_filename("main.rs");
    let meta = MemoryMeta {
        language: Some("en".into()),
        ..MemoryMeta::default()
    };
    let (_, _, _, meta) = parts(document_item(&chain, &document, meta).await.unwrap());
    assert_eq!(meta.language.as_deref(), Some("en"));
}

#[tokio::test]
async fn a_text_file_declared_plain_still_gets_its_language_from_the_extension() {
    let chain = ConverterChain::default();
    let document = RawDocument::new("x = 1")
        .with_filename("config.py")
        .with_mime("text/plain");
    let (_, _, mime, meta) = parts(
        document_item(&chain, &document, MemoryMeta::default())
            .await
            .unwrap(),
    );
    assert_eq!(mime.as_deref(), Some("text/plain"));
    assert_eq!(meta.language.as_deref(), Some("python"));
}

#[tokio::test]
async fn markdown_takes_its_title_from_the_first_heading() {
    let chain = ConverterChain::default();
    let document = RawDocument::new("# Plan\n\nShip it.").with_filename("notes/plan.md");
    let (title, body, mime, _) = parts(
        document_item(&chain, &document, MemoryMeta::default())
            .await
            .unwrap(),
    );
    assert_eq!(title.as_deref(), Some("Plan"));
    assert_eq!(body, "# Plan\n\nShip it.");
    assert_eq!(mime.as_deref(), Some("text/markdown"));
}

#[tokio::test]
async fn untitled_prose_falls_back_to_the_file_name_then_the_origin() {
    let chain = ConverterChain::default();
    let named = RawDocument::new("just prose").with_filename("dir/notes.txt");
    let (title, ..) = parts(
        document_item(&chain, &named, MemoryMeta::default())
            .await
            .unwrap(),
    );
    assert_eq!(title.as_deref(), Some("notes.txt"));

    let fetched = RawDocument::new("just prose").with_origin("https://example.com/a");
    let (title, ..) = parts(
        document_item(&chain, &fetched, MemoryMeta::default())
            .await
            .unwrap(),
    );
    assert_eq!(title.as_deref(), Some("https://example.com/a"));
}

#[tokio::test]
async fn a_format_nothing_converts_is_an_error_not_an_empty_item() {
    let chain = ConverterChain::default();
    let pdf = RawDocument::new(b"%PDF-1.7\nx".to_vec());
    let error = document_item(&chain, &pdf, MemoryMeta::default())
        .await
        .unwrap_err();
    assert!(
        matches!(error, Error::UnsupportedFormat(_)),
        "got {error:?}"
    );
}
