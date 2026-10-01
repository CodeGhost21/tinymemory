//! Tests for the surrounding module.

use super::*;

#[tokio::test]
async fn static_chat_provider_returns_response_and_counts() {
    let p = StaticChatProvider::new("hello");
    let prompt = ChatPrompt {
        system: "sys".into(),
        user: "u".into(),
        temperature: 0.0,
        kind: "test",
        max_tokens: None,
    };
    assert_eq!(p.chat_for_json(&prompt).await.unwrap(), "hello");
    assert_eq!(p.calls.load(std::sync::atomic::Ordering::SeqCst), 1);
}
