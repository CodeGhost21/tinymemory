use super::AnswerRequest;

#[test]
fn answer_request_defaults_bound_retrieval() {
    let request = AnswerRequest::new("what did we decide?");
    assert_eq!(request.limit, 12);
    assert_eq!(request.query, "what did we decide?");
    assert!(request.scope.is_none());
}
