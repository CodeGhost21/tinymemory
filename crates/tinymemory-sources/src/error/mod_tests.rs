//! Tests for the crate error and its mapping onto the contract error.

use super::*;

#[test]
fn a_reader_diagnostic_is_reported_verbatim() {
    let error = Error::Reader("page returned 404 Not Found".into());
    assert_eq!(error.to_string(), "page returned 404 Not Found");
}

#[test]
fn every_variant_maps_onto_the_contract_error_a_host_can_act_on() {
    use tinymemory_api::Error as Api;

    let cases: Vec<(Error, fn(&Api) -> bool)> = vec![
        (Error::Invalid("x".into()), |e| {
            matches!(e, Api::InvalidRequest(_))
        }),
        (Error::PathEscape("x".into()), |e| {
            matches!(e, Api::InvalidRequest(_))
        }),
        (Error::TooLarge("x".into()), |e| {
            matches!(e, Api::InvalidRequest(_))
        }),
        (Error::NotFound("x".into()), |e| matches!(e, Api::NotFound(_))),
        (Error::Unreachable("x".into()), |e| {
            matches!(e, Api::Unavailable(_))
        }),
        (Error::Registry("x".into()), |e| matches!(e, Api::Config(_))),
        (Error::Upstream("x".into()), |e| matches!(e, Api::Engine(_))),
        (Error::Reader("x".into()), |e| matches!(e, Api::Engine(_))),
        (Error::Io(std::io::Error::other("disk")), |e| {
            matches!(e, Api::Engine(_))
        }),
        (
            Error::Document(tinymemory_documents::Error::UnsupportedFormat("pdf".into())),
            |e| matches!(e, Api::Unsupported(_)),
        ),
    ];
    for (error, expected) in cases {
        let label = format!("{error:?}");
        assert!(expected(&Api::from(error)), "{label}");
    }

    let json = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    assert!(matches!(
        Api::from(Error::Json(json)),
        Api::InvalidRequest(_)
    ));
}
