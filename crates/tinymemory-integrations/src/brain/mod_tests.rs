//! Converting raw files into brain documents.

use super::*;
use crate::documents::ConverterChain;

#[test]
fn every_format_has_a_source() {
    assert_eq!(source_for(DocumentFormat::Pdf), BrainSource::Pdf);
    assert_eq!(source_for(DocumentFormat::PlainText), BrainSource::Markdown);
    assert_eq!(source_for(DocumentFormat::Html), BrainSource::Web);
    assert_eq!(
        source_for(DocumentFormat::Xlsx),
        BrainSource::Other("xlsx".into())
    );
    assert_eq!(
        source_for(DocumentFormat::Unknown).to_string(),
        "other"
    );
}

#[tokio::test]
async fn html_lands_on_the_web_unless_the_caller_says_otherwise() {
    let page = RawDocument::new("<html><head><title>Pricing</title></head><body><p>Pro is $20.</p></body></html>")
        .with_filename("pricing.html");
    let chain = ConverterChain::default();
    let document = brain_document(&chain, &page, None, MemoryMeta::default())
        .await
        .unwrap();
    assert_eq!(document.source, BrainSource::Web);
    assert_eq!(document.mime.as_deref(), Some("text/html"));
    assert!(document.text.contains("Pro is $20."));

    let notion = brain_document(&chain, &page, Some(BrainSource::Notion), MemoryMeta::default())
        .await
        .unwrap();
    assert_eq!(notion.source, BrainSource::Notion);
}

#[tokio::test]
async fn the_caller_s_metadata_is_kept_and_language_filled() {
    let file = RawDocument::new("fn main() {}\n").with_filename("src/main.rs");
    let meta = MemoryMeta {
        repo: Some("acme/app".into()),
        ..MemoryMeta::default()
    };
    let document = brain_document(&ConverterChain::default(), &file, None, meta)
        .await
        .unwrap();
    assert_eq!(document.source, BrainSource::Other("code".into()));
    assert_eq!(document.meta.repo.as_deref(), Some("acme/app"));
    assert_eq!(document.meta.language.as_deref(), Some("rust"));
}

#[tokio::test]
async fn an_empty_file_is_refused() {
    let error = brain_document(
        &ConverterChain::default(),
        &RawDocument::new("").with_filename("empty.md"),
        None,
        MemoryMeta::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(error, Error::Invalid(_)), "{error:?}");
}
