//! Splitting a document: exact reassembly, structure, pages and the limits.

use super::*;

/// The pieces of `text` under a small overhead and the given sizes,
/// checked to concatenate back to `text`.
fn pieces(text: &str, target: usize, limit: usize) -> Vec<Piece<'_>> {
    let pieces = split(text, 10, target, limit);
    let joined: String = pieces.iter().map(|piece| piece.text).collect();
    assert_eq!(joined, text, "the pieces concatenate to the text");
    pieces
}

#[test]
fn a_text_that_fits_is_one_piece() {
    let text = "# Title\n\nShort body.\n";
    let one = pieces(text, 1000, 10_000);
    assert_eq!(one.len(), 1);
    assert_eq!(one[0].text, text);
    assert_eq!(one[0].pages, None);
}

#[test]
fn sections_are_packed_up_to_the_target_and_named_by_their_heading() {
    let section = |title: &str| format!("## {title}\n\n{}\n\n", "word ".repeat(40));
    let text = format!(
        "{}{}{}{}",
        section("Alpha"),
        section("Beta"),
        section("Gamma"),
        section("Delta")
    );
    let packed = pieces(&text, 500, 10_000);
    assert!(packed.len() >= 2 && packed.len() < 4, "{}", packed.len());
    for piece in &packed {
        assert!(escaped_len(piece.text) + 10 <= 500, "{}", piece.text.len());
        assert!(
            piece.text.starts_with("## "),
            "cut at a heading: {:?}",
            piece.text
        );
    }
    assert_eq!(packed[0].section.as_deref(), Some("Alpha"));
    let sections: Vec<_> = packed.iter().filter_map(|p| p.section.as_deref()).collect();
    assert!(sections.windows(2).all(|w| w[0] != w[1]), "{sections:?}");
}

#[test]
fn at_a_zero_target_every_section_is_its_own_piece() {
    let text = "intro\n# One\nfirst\n# Two\nsecond\n# Three\nthird\n";
    let each = pieces(text, 0, 10_000);
    let sections: Vec<_> = each.iter().map(|p| p.section.as_deref()).collect();
    assert_eq!(
        sections,
        [None, Some("One"), Some("Two"), Some("Three")],
        "{each:?}"
    );
}

#[test]
fn page_breaks_number_the_pieces_and_never_split_a_page_across_units() {
    let page = |n: u32| format!("Page {n} text. {}\n", "x".repeat(60));
    let text = format!(
        "{}{PAGE_BREAK}{}{PAGE_BREAK}{}{PAGE_BREAK}{}",
        page(1),
        page(2),
        page(3),
        page(4)
    );
    let each = pieces(&text, 0, 10_000);
    let pages: Vec<_> = each.iter().map(|p| p.pages).collect();
    assert_eq!(
        pages,
        [Some((1, 1)), Some((2, 2)), Some((3, 3)), Some((4, 4))]
    );
    assert!(each[2].text.contains("Page 3"), "{:?}", each[2].text);
    let packed = pieces(&text, 200, 10_000);
    assert!(packed.len() > 1 && packed.len() < 4, "{packed:?}");
    assert_eq!(packed[0].pages.map(|(first, _)| first), Some(1));
    assert_eq!(packed.last().unwrap().pages.map(|(_, last)| last), Some(4));
    let one = pieces(&text, 10_000, 10_000);
    assert_eq!(
        one[0].pages,
        Some((1, 4)),
        "a paged text that fits spans all"
    );
}

#[test]
fn a_page_break_alone_never_becomes_a_piece() {
    let text = format!("First page.\n{PAGE_BREAK}# Heading\nSecond page.\n");
    let each = pieces(&text, 0, 10_000);
    assert_eq!(each.len(), 2, "{each:?}");
    assert_eq!(each[1].pages, Some((2, 2)));
    assert_eq!(each[1].section.as_deref(), Some("Heading"));
}

#[test]
fn an_oversized_unit_is_cut_at_paragraphs_then_lines_then_characters() {
    let paragraphs = format!("{}\n\n", "p".repeat(80)).repeat(10);
    for piece in pieces(&paragraphs, 0, 300) {
        assert!(escaped_len(piece.text) + 10 <= 300);
        assert!(piece.text.ends_with("\n\n"), "cut after a blank line");
    }
    let lines = format!("{}\n", "l".repeat(80)).repeat(10);
    for piece in pieces(&lines, 0, 300) {
        assert!(escaped_len(piece.text) + 10 <= 300);
        assert!(piece.text.ends_with('\n'), "cut after a line end");
    }
    let one_line = "é".repeat(1000);
    let cut = pieces(&one_line, 0, 300);
    assert!(cut.len() > 1);
    for piece in &cut {
        assert!(escaped_len(piece.text) + 10 <= 300);
    }
}

#[test]
fn sizes_count_json_escaping() {
    assert_eq!(escaped_len("ab"), 2);
    assert_eq!(escaped_len("\"\\\n"), 6);
    assert_eq!(escaped_len("\u{1}"), 6);
    assert_eq!(escaped_len("é"), 2);
    let quotes = "\"".repeat(400);
    for piece in pieces(&quotes, 0, 300) {
        assert!(
            escaped_len(piece.text) + 10 <= 300,
            "escaped, not raw, size"
        );
    }
}

#[test]
fn heading_lines_are_markdown_atx_headings_only() {
    assert_eq!(heading("# Title\nbody").as_deref(), Some("Title"));
    assert_eq!(heading("   ### Deep ###").as_deref(), Some("Deep"));
    assert_eq!(heading("#hashtag"), None);
    assert_eq!(heading("####### seven"), None);
    assert_eq!(heading("    # indented code"), None);
    assert_eq!(heading("# "), None);
    let long = format!("# {}", "t".repeat(500));
    assert_eq!(heading(&long).unwrap().chars().count(), MAX_SECTION_CHARS);
}
