use super::MemoryApi;
use tinymemory_api::capabilities::Capability;
use tinymemory_api::error::MemoryError;
use tinymemory_api::null::NullMemoryProvider;
use tinymemory_api::operations::AnswerRequest;

#[tokio::test]
async fn absent_optional_routes_return_the_named_capability() {
    let provider = NullMemoryProvider::new();
    let api = MemoryApi::new(&provider);
    let result = api.answer(AnswerRequest::new("question")).await;
    if let Err(error) = result {
        assert!(matches!(
            error,
            MemoryError::Unsupported {
                capability
            } if capability == Capability::Answer.as_str()
        ));
    } else {
        assert!(result.is_err(), "answer route should be absent");
    }
}
