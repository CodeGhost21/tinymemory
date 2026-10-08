//! Public reader-dispatch policy tests.

use tinymemory_integrations::sources::{
    SourceKind,
    readers::{is_locally_readable, reader_for},
};

#[test]
fn dispatch_constructs_the_local_readers() {
    for kind in [
        SourceKind::Folder,
        SourceKind::File,
        SourceKind::Conversation,
    ] {
        assert!(is_locally_readable(&kind));
        assert_eq!(reader_for(&kind).map(|reader| reader.kind()), Some(kind));
    }
}

#[test]
fn every_kind_has_a_local_reader() {
    for kind in SourceKind::ALL {
        assert!(is_locally_readable(&kind));
        assert!(reader_for(&kind).is_some());
    }
}
