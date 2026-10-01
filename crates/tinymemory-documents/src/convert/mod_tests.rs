//! Tests for the converter seam.

use super::*;

fn raw(bytes: &str, mime: &str) -> RawDocument {
    RawDocument::new(bytes).with_mime(mime)
}

/// A converter that claims one format and always succeeds, for chain-ordering
/// tests.
struct Stub {
    name: &'static str,
    format: DocumentFormat,
}

#[async_trait]
impl DocumentConverter for Stub {
    fn name(&self) -> &str {
        self.name
    }

    fn supports(&self, format: DocumentFormat) -> bool {
        format == self.format
    }

    async fn convert(&self, document: &RawDocument) -> Result<ConvertedDocument> {
        check_size(document)?;
        Ok(ConvertedDocument::new(
            format!("from {}", self.name),
            self.format,
            document.bytes.len(),
        )
        .with_title(Some("Stubbed".to_string())))
    }
}

/// A converter that claims a format and then fails, to prove a chain does not
/// silently fall through to a converter that already declined.
struct Failing;

#[async_trait]
impl DocumentConverter for Failing {
    fn name(&self) -> &str {
        "failing"
    }

    fn supports(&self, format: DocumentFormat) -> bool {
        format == DocumentFormat::Pdf
    }

    async fn convert(&self, _document: &RawDocument) -> Result<ConvertedDocument> {
        Err(MemoryError::Backend("extractor crashed".to_string()))
    }
}

#[tokio::test]
async fn markdown_passes_through_untouched() {
    let source = "# Title\n\nSome *prose*.\n";
    let converted = NativeConverter
        .convert(&raw(source, "text/markdown"))
        .await
        .unwrap();
    assert_eq!(converted.markdown, source);
    assert_eq!(converted.format, DocumentFormat::Markdown);
    assert_eq!(converted.source_bytes, source.len());
}

#[tokio::test]
async fn plain_text_is_stored_as_written_rather_than_reformatted() {
    let source = "line one\nline two\n   indented";
    let converted = NativeConverter
        .convert(&raw(source, "text/plain"))
        .await
        .unwrap();
    assert_eq!(converted.markdown, source);
    assert_eq!(converted.format, DocumentFormat::PlainText);
}

#[tokio::test]
async fn html_is_converted_and_its_title_recovered() {
    let source =
        "<html><head><title>Notes</title></head><body><h1>Heading</h1><p>Body.</p></body></html>";
    let converted = NativeConverter
        .convert(&raw(source, "text/html"))
        .await
        .unwrap();
    assert_eq!(converted.markdown, "# Heading\n\nBody.");
    assert_eq!(converted.title.as_deref(), Some("Notes"));
    assert_eq!(converted.format, DocumentFormat::Html);
}

#[tokio::test]
async fn the_converter_records_its_own_name_in_metadata() {
    let converted = NativeConverter
        .convert(&raw("text", "text/plain"))
        .await
        .unwrap();
    assert_eq!(converted.metadata["converter"], "native");
}

#[tokio::test]
async fn a_pdf_is_refused_with_an_error_that_says_what_is_missing() {
    let pdf = RawDocument::new(b"%PDF-1.7\ncontent".to_vec());
    let error = NativeConverter.convert(&pdf).await.unwrap_err();
    assert!(matches!(error, MemoryError::Invalid(_)), "got {error:?}");
    assert!(error.to_string().contains("pdf"), "got {error}");
}

#[tokio::test]
async fn an_empty_document_is_rejected() {
    let error = NativeConverter
        .convert(&RawDocument::new(Vec::new()))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("empty"), "got {error}");
}

#[tokio::test]
async fn a_document_over_the_cap_is_a_budget_error_not_a_validation_one() {
    let oversized = RawDocument::new(vec![b'a'; MAX_DOCUMENT_BYTES + 1]).with_mime("text/plain");
    let error = NativeConverter.convert(&oversized).await.unwrap_err();
    assert!(
        matches!(error, MemoryError::BudgetExceeded(_)),
        "got {error:?}"
    );
}

#[tokio::test]
async fn a_document_of_exactly_the_cap_is_accepted() {
    let at_cap = RawDocument::new(vec![b'a'; MAX_DOCUMENT_BYTES]).with_mime("text/plain");
    assert!(NativeConverter.convert(&at_cap).await.is_ok());
}

#[tokio::test]
async fn invalid_utf8_in_a_textual_format_is_rejected() {
    let bad = RawDocument::new(vec![b'h', b'i', 0xFF]).with_mime("text/plain");
    let error = NativeConverter.convert(&bad).await.unwrap_err();
    assert!(error.to_string().contains("utf-8"), "got {error}");
}

#[tokio::test]
async fn html_that_converts_to_nothing_is_an_error_not_an_empty_document() {
    let empty = raw(
        "<html><head><style>p{}</style></head><body></body></html>",
        "text/html",
    );
    let error = NativeConverter.convert(&empty).await.unwrap_err();
    assert!(error.to_string().contains("no text"), "got {error}");
}

#[tokio::test]
async fn the_default_chain_converts_the_three_native_formats_and_nothing_else() {
    let chain = ConverterChain::default();
    assert_eq!(
        chain.supported_formats(),
        vec![
            DocumentFormat::Markdown,
            DocumentFormat::PlainText,
            DocumentFormat::Html
        ]
    );
    assert!(!chain.supports(DocumentFormat::Pdf));
}

#[tokio::test]
async fn a_chain_uses_the_first_converter_that_claims_the_format() {
    let chain = ConverterChain::new(vec![
        Box::new(Stub {
            name: "first",
            format: DocumentFormat::Html,
        }),
        Box::new(Stub {
            name: "second",
            format: DocumentFormat::Html,
        }),
    ]);
    let converted = chain.convert(&raw("<p>x</p>", "text/html")).await.unwrap();
    assert_eq!(converted.markdown, "from first");
}

#[tokio::test]
async fn prepending_a_converter_puts_it_ahead_of_the_native_one() {
    let chain = ConverterChain::default().prepend(Box::new(Stub {
        name: "custom",
        format: DocumentFormat::Html,
    }));
    let converted = chain
        .convert(&raw("<h1>real</h1>", "text/html"))
        .await
        .unwrap();
    assert_eq!(converted.markdown, "from custom");
}

#[tokio::test]
async fn appending_a_converter_extends_what_the_chain_handles() {
    let chain = ConverterChain::default().push(Box::new(Stub {
        name: "pdf",
        format: DocumentFormat::Pdf,
    }));
    assert!(chain.supports(DocumentFormat::Pdf));
    let converted = chain
        .convert(&RawDocument::new(b"%PDF-1.7\nx".to_vec()))
        .await
        .unwrap();
    assert_eq!(converted.markdown, "from pdf");
    // The native converter still owns the formats it already handled.
    let html = chain
        .convert(&raw("<h1>real</h1>", "text/html"))
        .await
        .unwrap();
    assert_eq!(html.markdown, "# real");
}

#[tokio::test]
async fn a_chain_does_not_fall_through_when_its_chosen_converter_fails() {
    let chain = ConverterChain::new(vec![
        Box::new(Failing),
        Box::new(Stub {
            name: "fallback",
            format: DocumentFormat::Pdf,
        }),
    ]);
    let error = chain
        .convert(&RawDocument::new(b"%PDF-1.7\nx".to_vec()))
        .await
        .unwrap_err();
    assert!(matches!(error, MemoryError::Backend(_)), "got {error:?}");
}

#[tokio::test]
async fn an_unhandled_format_names_what_the_build_can_convert() {
    let chain = ConverterChain::default();
    let error = chain
        .convert(&RawDocument::new(b"%PDF-1.7\nx".to_vec()))
        .await
        .unwrap_err();
    let message = error.to_string();
    assert!(message.contains("pdf"), "{message}");
    assert!(message.contains("markdown"), "{message}");
}

#[tokio::test]
async fn an_empty_chain_says_it_converts_nothing() {
    let chain = ConverterChain::new(Vec::new());
    let error = chain.convert(&raw("text", "text/plain")).await.unwrap_err();
    assert!(error.to_string().contains("nothing"), "got {error}");
}

#[test]
fn a_raw_document_detects_its_own_format_from_what_it_carries() {
    assert_eq!(
        RawDocument::new("x").with_mime("text/html").format(),
        DocumentFormat::Html
    );
    assert_eq!(
        RawDocument::new("x").with_filename("a.md").format(),
        DocumentFormat::Markdown
    );
}

#[test]
fn a_display_name_prefers_the_filename_then_the_origin() {
    let named = RawDocument::new("x")
        .with_filename("report.pdf")
        .with_origin("https://example.com/report.pdf");
    assert_eq!(named.display_name(), "report.pdf");

    let fetched = RawDocument::new("x").with_origin("https://example.com/page");
    assert_eq!(fetched.display_name(), "https://example.com/page");

    let anonymous = RawDocument::new("plain text");
    assert_eq!(anonymous.display_name(), "document.txt");
}

#[test]
fn title_or_falls_back_to_the_first_heading_before_the_supplied_default() {
    let converted = ConvertedDocument::new("# Real Title\n\nbody", DocumentFormat::Markdown, 20);
    assert_eq!(converted.title_or("upload.md"), "Real Title");

    let untitled = ConvertedDocument::new("just body text", DocumentFormat::PlainText, 14);
    assert_eq!(untitled.title_or("upload.txt"), "upload.txt");
}

#[test]
fn an_explicit_title_wins_over_a_heading() {
    let converted = ConvertedDocument::new("# Heading", DocumentFormat::Markdown, 9)
        .with_title(Some("Explicit".to_string()));
    assert_eq!(converted.title_or("fallback"), "Explicit");
}

#[test]
fn a_blank_title_is_treated_as_no_title() {
    let converted = ConvertedDocument::new("body", DocumentFormat::PlainText, 4)
        .with_title(Some("   ".to_string()));
    assert_eq!(converted.title, None);
}

#[test]
fn an_empty_heading_is_not_mistaken_for_a_title() {
    let converted = ConvertedDocument::new("#\n\nbody", DocumentFormat::Markdown, 7);
    assert_eq!(converted.title_or("fallback"), "fallback");
}
