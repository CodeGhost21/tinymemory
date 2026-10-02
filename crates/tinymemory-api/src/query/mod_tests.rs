//! Request validation and wire shapes.

use super::*;

#[test]
fn malformed_requests_are_refused() {
    assert!(RecallRequest::new(" ", 3).validate().is_err());
    assert!(RecallRequest::new("q", 0).validate().is_err());
    assert!(RecallRequest::new("q", 1).validate().is_ok());
    assert!(FetchRequest::new("", FetchMode::Hybrid, 3).validate().is_err());
    assert!(FetchRequest::new("q", FetchMode::Hybrid, 0).validate().is_err());
    assert!(FetchRequest::new("q", FetchMode::Keyword, 2).validate().is_ok());
    assert!(ListRequest::new(MetaFilter::default(), 0).validate().is_err());
    assert!(ListRequest::new(MetaFilter::default(), 1).validate().is_ok());
}

#[test]
fn a_forget_can_never_mean_everything() {
    assert!(matches!(
        ForgetTarget::Ids(Vec::new()).validate(),
        Err(Error::InvalidRequest(_))
    ));
    assert!(matches!(
        ForgetTarget::Filter(MetaFilter::default()).validate(),
        Err(Error::InvalidRequest(_))
    ));
    assert!(ForgetTarget::Ids(vec![ItemId::from("a")]).validate().is_ok());
    assert!(
        ForgetTarget::Filter(MetaFilter::kinds([ItemKind::Learning]))
            .validate()
            .is_ok()
    );
}

#[test]
fn fetch_mode_wire_strings_match_serde() {
    for mode in FetchMode::ALL {
        assert_eq!(serde_json::to_value(mode).expect("json"), mode.as_str());
    }
}

#[test]
fn requests_deserialise_with_defaults() {
    let request: FetchRequest =
        serde_json::from_str(r#"{"query":"q","mode":"hybrid","limit":2}"#).expect("parse");
    assert!(request.filter.is_empty());
    assert_eq!(request.cursor, None);
}
