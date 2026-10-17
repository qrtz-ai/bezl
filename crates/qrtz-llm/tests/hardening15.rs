//! Hardening cycle 15 (N81, ADR 0053): the LLM client follows no redirect and reads a bounded reply.
mod common;

use std::time::Duration;

use common::{Mock, Reply};
use qrtz_llm::prompt::{ChatMessage, Role};
use qrtz_llm::{create_provider, http_client, Secret};

const KEY: &str = "sk-ant-secretsecret";

fn msgs() -> Vec<ChatMessage> {
    vec![ChatMessage {
        role: Role::User,
        content: "the whole transcript".into(),
    }]
}

#[tokio::test]
async fn n81_a_redirect_is_not_followed_so_neither_key_nor_transcript_leaves() {
    for (provider, status) in [
        ("anthropic", 307),
        ("google", 308),
        ("openrouter", 307),
        ("anthropic", 302),
    ] {
        let (api, elsewhere) = (Mock::start().await, Mock::start().await);
        api.push(Reply::Redirect {
            status,
            location: format!("{}/steal", elsewhere.base_url),
        });
        let p = create_provider(
            &http_client(),
            provider,
            Some(Secret::new(KEY)),
            None,
            Some(&api.base_url),
        )
        .unwrap();
        let e = p.complete(&msgs(), true).await.unwrap_err();
        assert!(!e.retryable, "{provider} {status}: {e:?}");
        assert!(e.message.contains("redirect not followed"), "{}", e.message);
        assert!(!e.message.contains(&elsewhere.base_url), "{}", e.message);
        assert_eq!(api.seen().len(), 1);
        assert!(
            elsewhere.seen().is_empty(),
            "{provider} {status}: followed to {:?}",
            elsewhere.seen()
        );
    }
}

#[tokio::test]
async fn n81_an_oversized_reply_is_refused_not_buffered() {
    let api = Mock::start().await;
    let huge = format!(
        r#"{{"choices":[{{"message":{{"content":"{}"}}}}]}}"#,
        "x".repeat(9 << 20)
    );
    api.push(Reply::json(200, huge));
    api.push(Reply::json(
        500,
        format!(r#"{{"error":"{}"}}"#, "y".repeat(1 << 20)),
    ));
    let p = create_provider(
        &http_client(),
        "openrouter",
        Some(Secret::new(KEY)),
        None,
        Some(&api.base_url),
    )
    .unwrap();
    let e = p.complete(&msgs(), true).await.unwrap_err();
    assert!(!e.retryable && e.message.contains("over 8 MiB"), "{e:?}");
    // A huge error body: only its start is read, and the class is still from the status.
    let e = tokio::time::timeout(Duration::from_secs(10), p.complete(&msgs(), true))
        .await
        .unwrap()
        .unwrap_err();
    assert!(
        e.retryable && e.message.starts_with("LLM HTTP 500"),
        "{}",
        &e.message[..60.min(e.message.len())]
    );
    assert!(e.message.len() < 400);
}

#[tokio::test]
async fn n81_a_normal_reply_still_reads() {
    let api = Mock::start().await;
    api.push(Reply::content("hello"));
    let p = create_provider(
        &http_client(),
        "openrouter",
        Some(Secret::new(KEY)),
        None,
        Some(&api.base_url),
    )
    .unwrap();
    assert_eq!(p.complete(&msgs(), false).await.unwrap(), "hello");
}
