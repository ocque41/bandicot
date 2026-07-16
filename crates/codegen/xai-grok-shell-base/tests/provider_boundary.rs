//! Production-surface provider trust-boundary regression tests.

use xai_grok_shell_base::util::is_first_party_xai_url;

#[test]
fn first_party_xai_boundary_requires_https_default_port_and_real_hostname() {
    assert!(is_first_party_xai_url("https://api.x.ai/v1"));
    assert!(is_first_party_xai_url(
        "https://cli-chat-proxy.grok.com/v1/responses"
    ));

    for untrusted in [
        "https://api.openai.com/v1",
        "http://api.x.ai/v1",
        "https://api.x.ai:8443/v1",
        "https://api.x.ai.evil.example/v1",
        "https://cli-chat-proxy.grok.com.evil.example/v1",
        "https://cli-chat-proxy.grok.com/v11/responses",
        "http://127.0.0.1:8000/v1",
        "not-a-url",
    ] {
        assert!(
            !is_first_party_xai_url(untrusted),
            "untrusted provider was classified as xAI: {untrusted}"
        );
    }
}
