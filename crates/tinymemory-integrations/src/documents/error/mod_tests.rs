//! Tests for the module error and its mapping onto the contract error.

use super::*;

#[test]
fn messages_are_lowercase_and_name_the_failure() {
    let too_large = Error::TooLarge { size: 10, limit: 5 };
    assert_eq!(
        too_large.to_string(),
        "document is 10 bytes, over the 5-byte intake limit"
    );
    let converter = Error::Converter {
        converter: "pdf".into(),
        message: "crashed".into(),
    };
    assert_eq!(converter.to_string(), "converter pdf failed: crashed");
}

#[test]
fn every_variant_maps_onto_a_contract_error() {
    assert!(matches!(
        tinymemory_api::Error::from(Error::Invalid("empty".into())),
        tinymemory_api::Error::InvalidRequest(_)
    ));
    assert!(matches!(
        tinymemory_api::Error::from(Error::TooLarge { size: 2, limit: 1 }),
        tinymemory_api::Error::InvalidRequest(_)
    ));
    assert!(matches!(
        tinymemory_api::Error::from(Error::UnsupportedFormat("pdf".into())),
        tinymemory_api::Error::Unsupported(_)
    ));
    assert!(matches!(
        tinymemory_api::Error::from(Error::Converter {
            converter: "x".into(),
            message: "y".into()
        }),
        tinymemory_api::Error::Engine(_)
    ));
}
