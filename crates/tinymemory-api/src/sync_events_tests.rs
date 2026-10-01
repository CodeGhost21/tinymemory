use super::extract_mem_src_id;

#[test]
fn extracts_the_registry_id_from_a_composite() {
    assert_eq!(
        extract_mem_src_id("mem_src:src-rss-42:https://example.com/item-7"),
        Some("src-rss-42")
    );
}

#[test]
fn takes_the_first_colon_so_item_ids_may_contain_colons() {
    // RSS GUIDs are routinely URLs. Splitting on the *last* colon would
    // return "src-rss-42:https" here.
    assert_eq!(
        extract_mem_src_id("mem_src:src-rss-42:https://example.com/a:b:c"),
        Some("src-rss-42")
    );
}

#[test]
fn rejects_a_non_composite_source_id() {
    // Channel / Composio scopes such as `slack:#eng` are not this shape.
    assert_eq!(extract_mem_src_id("slack:#eng"), None);
    assert_eq!(extract_mem_src_id("gmail:alice"), None);
}

#[test]
fn rejects_a_missing_or_empty_item_id() {
    assert_eq!(extract_mem_src_id("mem_src:src-rss-42"), None);
    assert_eq!(extract_mem_src_id("mem_src:src-rss-42:"), None);
}

/// `Some("")` is not a source id. It denied anyway, because the allowlist
/// it is tested against has empty entries stripped — but that made the
/// safety a property of the *caller*, not of this parse.
#[test]
fn rejects_an_empty_source_id() {
    assert_eq!(extract_mem_src_id("mem_src::item-7"), None);
    assert_eq!(extract_mem_src_id("mem_src::"), None);
}
