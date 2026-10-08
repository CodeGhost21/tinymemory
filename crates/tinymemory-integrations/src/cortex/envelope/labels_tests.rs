//! Tests for lookup labels.

use super::*;
use tinymemory_api::SourceKind;

#[test]
fn a_digest_is_sixteen_hex_digits_and_stable() {
    let d = digest("owner/repo");
    assert_eq!(d.len(), 16);
    assert!(d.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(d, digest("owner/repo"));
    assert_ne!(d, digest("owner/other"));
}

#[test]
fn an_item_carries_its_id_label_and_one_per_set_field() {
    let mut meta = MemoryMeta::from_source(SourceKind::Folder, Some("src-1".into()));
    meta.repo = Some("a/b".into());
    let labels = for_item("abc", &meta);
    assert_eq!(labels[0], item("abc"));
    assert!(labels.contains(&format!("tm:r:{}", digest("a/b"))));
    assert!(labels.contains(&format!("tm:s:{}", digest("src-1"))));
    assert!(labels.contains(&format!("tm:k:{}", digest("folder"))));
    assert_eq!(labels.len(), 4);
    assert!(labels.iter().all(|l| l.len() <= 24 && !l.contains(',')));
}

#[test]
fn a_filter_narrows_by_its_most_selective_labelled_field() {
    let filter = MetaFilter {
        repo: Some("a/b".into()),
        thread_id: Some("t1".into()),
        ..MetaFilter::default()
    };
    assert_eq!(
        narrowing(&filter),
        Some(vec![format!("tm:t:{}", digest("t1"))])
    );
    let by_sources = MetaFilter {
        sources: vec![SourceKind::File, SourceKind::Import],
        ..MetaFilter::default()
    };
    assert_eq!(narrowing(&by_sources).map(|l| l.len()), Some(2));
    let unlabelled = MetaFilter {
        folder: Some("/a".into()),
        ..MetaFilter::default()
    };
    assert_eq!(narrowing(&unlabelled), None);
}

#[test]
fn a_path_digest_is_as_long_as_an_item_id_and_extends_the_label_digest() {
    let path = "/Users/priya/Documents";
    let long = path_digest(path);
    assert_eq!(long.len(), 40);
    assert!(long.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(long[..16], digest(path));
    assert_eq!(workspace(&long), format!("tm:w:{}", digest(path)));
}
