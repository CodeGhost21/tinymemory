//! Tests for credential redaction and resolution.

use super::*;

#[test]
fn debug_never_shows_a_token() {
    let bearer = StaticBearer::new("tiny_live_secret");
    assert!(!format!("{bearer:?}").contains("secret"));
    let credential = CortexCredential::api_key("ctx_secret");
    assert!(!format!("{credential:?}").contains("secret"));
    let dynamic = CortexCredential::from(Arc::new(bearer) as Arc<dyn BearerSource>);
    assert!(!format!("{dynamic:?}").contains("secret"));
}

#[tokio::test]
async fn both_kinds_resolve_to_their_token() {
    assert_eq!(
        CortexCredential::api_key("k").resolve().await.unwrap(),
        "k"
    );
    let dynamic = CortexCredential::Dynamic(Arc::new(StaticBearer::new("t")));
    assert_eq!(dynamic.resolve().await.unwrap(), "t");
}
