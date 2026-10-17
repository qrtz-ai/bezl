//! N87: in local-only mode Ollama is used only while its URL is loopback; a base URL override naming another host
//! (`QRTZ_LLM_BASE_URL`) falls back to the on-device summary instead of sending the transcript off the machine.
use qrtz_llm::steps::{is_loopback_url, LlmConfig};

fn cfg(provider: &str, base: Option<&str>, local_only: bool) -> LlmConfig {
    LlmConfig {
        provider: provider.into(),
        model: String::new(),
        base_url: base.map(String::from),
        local_only,
    }
}

#[test]
fn local_only_keeps_ollama_only_on_loopback() {
    for ok in [
        None,
        Some("http://127.0.0.1:11434/v1"),
        Some("http://localhost:11434/v1"),
        Some("http://LOCALHOST:1/v1"),
        Some("http://127.8.9.10:11434"),
        Some("http://127.1:11434"),
        Some("http://[::1]:11434/v1"),
    ] {
        assert_eq!(cfg("ollama", ok, true).effective_provider(), "ollama", "{ok:?}");
    }
    for bad in [
        "http://10.0.0.5:11434/v1",
        "https://ollama.example.com/v1",
        "http://localhost.evil.com/v1",
        "http://127.0.0.1.nip.io/v1",
        "http://user:pw@127.0.0.1:11434/v1",
        "http://[::ffff:8.8.8.8]/v1",
        "http://0.0.0.0:11434",
        "file:///etc/passwd",
        "not a url",
    ] {
        assert_eq!(
            cfg("ollama", Some(bad), true).effective_provider(),
            "none",
            "{bad}"
        );
        assert!(!is_loopback_url(bad), "{bad}");
    }
    // Not local-only: the override is honoured (tests, a user's own gateway).
    assert_eq!(
        cfg("ollama", Some("http://10.0.0.5:11434/v1"), false).effective_provider(),
        "ollama"
    );
    assert_eq!(
        cfg("openrouter", Some("http://127.0.0.1:9"), true).effective_provider(),
        "none"
    );
    assert_eq!(
        cfg("none", Some("http://10.0.0.5"), true).effective_provider(),
        "none"
    );
}
