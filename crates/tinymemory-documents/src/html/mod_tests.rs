//! Tests for the HTML to markdown converter.

use super::*;

#[test]
fn headings_become_atx_headings_at_the_right_level() {
    assert_eq!(to_markdown("<h1>Title</h1>"), "# Title");
    assert_eq!(to_markdown("<h3>Sub</h3>"), "### Sub");
    assert_eq!(to_markdown("<h1>A</h1><h2>B</h2>"), "# A\n\n## B");
}

#[test]
fn paragraphs_are_separated_by_a_blank_line() {
    assert_eq!(to_markdown("<p>One</p><p>Two</p>"), "One\n\nTwo");
}

#[test]
fn consecutive_block_tags_do_not_pile_up_blank_lines() {
    let markdown = to_markdown("<div><div><p>Only</p></div></div>");
    assert_eq!(markdown, "Only");
}

#[test]
fn an_unordered_list_becomes_dashes() {
    assert_eq!(
        to_markdown("<ul><li>one</li><li>two</li></ul>"),
        "- one\n- two"
    );
}

#[test]
fn an_ordered_list_is_numbered_from_one() {
    assert_eq!(
        to_markdown("<ol><li>one</li><li>two</li><li>three</li></ol>"),
        "1. one\n2. two\n3. three"
    );
}

#[test]
fn a_nested_list_is_indented() {
    let markdown = to_markdown("<ul><li>outer</li><ul><li>inner</li></ul></ul>");
    assert!(markdown.contains("- outer"), "{markdown}");
    assert!(markdown.contains("  - inner"), "{markdown}");
}

#[test]
fn two_ordered_lists_each_start_at_one() {
    let markdown = to_markdown("<ol><li>a</li></ol><ol><li>b</li></ol>");
    assert_eq!(markdown.matches("1. ").count(), 2, "{markdown}");
}

#[test]
fn a_link_becomes_an_inline_markdown_link() {
    assert_eq!(
        to_markdown(r#"<a href="https://example.com">Example</a>"#),
        "[Example](https://example.com)"
    );
}

#[test]
fn a_single_quoted_or_unquoted_href_is_still_read() {
    assert_eq!(
        to_markdown("<a href='https://example.com'>Example</a>"),
        "[Example](https://example.com)"
    );
    assert_eq!(
        to_markdown("<a href=https://example.com>Example</a>"),
        "[Example](https://example.com)"
    );
}

#[test]
fn a_link_with_no_text_becomes_an_autolink() {
    assert_eq!(
        to_markdown(r#"<a href="https://example.com"></a>"#),
        "<https://example.com>"
    );
}

#[test]
fn an_anchor_without_an_href_keeps_its_text() {
    assert_eq!(to_markdown("<a name=\"x\">Anchor</a>"), "Anchor");
}

#[test]
fn an_unclosed_anchor_does_not_lose_its_text() {
    assert_eq!(
        to_markdown(r#"<p><a href="https://example.com">Example"#),
        "[Example](https://example.com)"
    );
}

#[test]
fn emphasis_and_strong_map_to_their_markdown_forms() {
    assert_eq!(to_markdown("<strong>bold</strong>"), "**bold**");
    assert_eq!(to_markdown("<b>bold</b>"), "**bold**");
    assert_eq!(to_markdown("<em>italic</em>"), "*italic*");
    assert_eq!(to_markdown("<i>italic</i>"), "*italic*");
}

#[test]
fn inline_code_is_backticked() {
    assert_eq!(to_markdown("<code>let x = 1;</code>"), "`let x = 1;`");
}

#[test]
fn a_pre_block_becomes_a_fence_and_keeps_its_whitespace() {
    let markdown = to_markdown("<pre>fn main() {\n    ok();\n}</pre>");
    assert!(markdown.starts_with("```"), "{markdown}");
    assert!(markdown.contains("    ok();"), "{markdown}");
}

#[test]
fn code_inside_a_pre_block_is_not_double_backticked() {
    let markdown = to_markdown("<pre><code>x = 1</code></pre>");
    assert!(!markdown
        .contains('`')
        .then(|| markdown.contains("`x"))
        .unwrap_or(false));
    assert!(markdown.contains("x = 1"), "{markdown}");
}

#[test]
fn a_line_break_becomes_a_hard_break() {
    assert_eq!(to_markdown("a<br>b"), "a  \nb");
}

#[test]
fn a_horizontal_rule_becomes_a_thematic_break() {
    assert_eq!(to_markdown("<p>a</p><hr><p>b</p>"), "a\n\n---\n\nb");
}

#[test]
fn a_block_quote_is_prefixed() {
    assert_eq!(to_markdown("<blockquote>quoted</blockquote>"), "> quoted");
}

#[test]
fn script_and_style_bodies_never_reach_the_output() {
    let html =
        r#"<style>body{color:red}</style><script>var a = "<p>fake</p>";</script><p>real</p>"#;
    let markdown = to_markdown(html);
    assert_eq!(markdown, "real");
}

#[test]
fn an_unclosed_script_swallows_the_rest_of_the_document() {
    let markdown = to_markdown("<p>before</p><script>var x = 1;<p>after</p>");
    assert_eq!(markdown, "before");
}

#[test]
fn comments_are_removed() {
    assert_eq!(to_markdown("<p>a</p><!-- hidden --><p>b</p>"), "a\n\nb");
}

#[test]
fn entities_in_text_are_decoded() {
    assert_eq!(
        to_markdown("<p>Tom &amp; Jerry &hellip;</p>"),
        "Tom & Jerry …"
    );
}

#[test]
fn whitespace_between_inline_runs_is_preserved_as_a_word_boundary() {
    assert_eq!(to_markdown("<span>one</span> <span>two</span>"), "one two");
}

#[test]
fn whitespace_between_block_tags_is_dropped() {
    assert_eq!(to_markdown("<p>one</p>\n\n   \n<p>two</p>"), "one\n\ntwo");
}

#[test]
fn runs_of_whitespace_inside_a_paragraph_collapse() {
    assert_eq!(to_markdown("<p>one    two\n\tthree</p>"), "one two three");
}

#[test]
fn an_unterminated_tag_is_treated_as_text() {
    // The `</p>` closed a block, so the break before the literal text is
    // correct; what matters is that the unterminated tag is not swallowed.
    assert_eq!(to_markdown("<p>ok</p><notclosed"), "ok\n\n<notclosed");
    assert_eq!(to_markdown("plain <notclosed"), "plain <notclosed");
}

#[test]
fn a_document_with_no_markup_at_all_survives() {
    assert_eq!(to_markdown("just prose"), "just prose");
}

#[test]
fn an_empty_document_produces_an_empty_string() {
    assert_eq!(to_markdown(""), "");
}

#[test]
fn a_whole_page_converts_end_to_end() {
    let html = r#"
        <!DOCTYPE html>
        <html><head><title>Notes</title><style>p{}</style></head>
        <body>
          <h1>Notes</h1>
          <p>An <strong>important</strong> point with a
             <a href="https://example.com">link</a>.</p>
          <ul><li>first</li><li>second</li></ul>
        </body></html>"#;
    let markdown = to_markdown(html);
    assert!(markdown.contains("# Notes"), "{markdown}");
    assert!(
        markdown.contains("An **important** point with a [link](https://example.com)."),
        "{markdown}"
    );
    assert!(markdown.contains("- first\n- second"), "{markdown}");
    assert!(!markdown.contains("p{}"), "{markdown}");
}

#[test]
fn a_title_is_extracted_and_decoded() {
    assert_eq!(
        extract_title("<html><head><title>A &amp; B</title></head></html>"),
        Some("A & B".to_string())
    );
}

#[test]
fn a_title_with_attributes_is_still_found() {
    assert_eq!(
        extract_title(r#"<TITLE lang="en">Mixed Case</TITLE>"#),
        Some("Mixed Case".to_string())
    );
}

#[test]
fn a_missing_or_empty_title_is_none() {
    assert_eq!(extract_title("<html><body>no title</body></html>"), None);
    assert_eq!(extract_title("<title>   </title>"), None);
}

#[test]
fn elements_whose_names_merely_start_with_a_raw_element_name_keep_their_text() {
    // `<scripture>` and `<style-guide>` share a prefix with `script`/`style`
    // but are not those elements; only `>`, `/`, or whitespace right after the
    // name is a real tag-name boundary.
    assert_eq!(to_markdown("<scripture>keep</scripture>"), "keep");
    assert_eq!(to_markdown("<style-guide>keep</style-guide>"), "keep");
}
