//! Public reader-dispatch policy tests.

use tinymemory_integrations::sources::{
    SourceKind,
    readers::{is_locally_readable, reader_for},
};

#[test]
fn timer_dispatch_constructs_only_readers_that_never_need_network() {
    for kind in [
        SourceKind::Folder,
        SourceKind::File,
        SourceKind::Conversation,
    ] {
        assert!(is_locally_readable(&kind));
        assert_eq!(reader_for(&kind).map(|reader| reader.kind()), Some(kind));
    }

    for kind in [
        SourceKind::GithubRepo,
        SourceKind::RssFeed,
        SourceKind::WebPage,
    ] {
        assert!(!is_locally_readable(&kind));
        assert!(reader_for(&kind).is_none());
    }
}

#[cfg(feature = "sources-network")]
#[test]
fn request_dispatch_hands_out_a_reader_for_every_kind() {
    use tinymemory_integrations::sources::readers::reader_for_request;

    for kind in SourceKind::ALL {
        assert_eq!(reader_for_request(&kind).kind(), kind);
    }
}
