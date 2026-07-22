//! # API Key Fallback Provider Integration Example
//!
//! This example shows how to integrate the fallback provider with the session layer
//! to automatically switch between API keys when one fails.
//!
//! ## Configuration
//!
//! Set the `GROK_FALLBACK_KEYS` environment variable with a JSON array of providers:
//!
//! ```bash
//! export GROK_FALLBACK_KEYS='{
//!   "providers": [
//!     {"name": "zen-a", "api_key": "sk-FAKE-BANDICOT-EXAMPLE-KEY"},
//!     {"name": "go-a", "api_key": "sk-FAKE-BANDICOT-EXAMPLE-KEY"},
//!     {"name": "zen-b", "api_key": "sk-FAKE-BANDICOT-EXAMPLE-KEY"}
//!   ],
//!   "recovery_interval_secs": 300,
//!   "max_failures": 3
//! }'
//! ```
//!
//! ## Usage in Session
//!
//! ```rust
//! use xai_grok_tools::types::fallback_wrapper::FallbackWrapper;
//!
//! // Create fallback wrapper from environment
//! let fallback = FallbackWrapper::from_env();
//!
//! // Use in session initialization
//! if let Some(wrapper) = fallback {
//!     // Get current API key
//!     let api_key = wrapper.current_api_key();
//!     
//!     // After successful request
//!     wrapper.record_success();
//!     
//!     // After failed request (rate limit or auth error)
//!     wrapper.record_failure(true, false); // is_rate_limit, is_auth_error
//! }
//! ```

use xai_grok_tools::types::{
    fallback_provider::{AuthScheme, FallbackApiKeyProvider, FallbackConfig, ProviderEntry},
    fallback_wrapper::FallbackWrapper,
};

/// Example: Create a fallback provider chain with your API keys.
fn example_create_fallback_provider() {
    let providers = vec![
        ProviderEntry {
            name: "zen-account-a".to_string(),
            api_key: "sk-FAKE-BANDICOT-EXAMPLE-KEY".to_string(),
            base_url: None,
            auth_scheme: AuthScheme::Bearer,
        },
        ProviderEntry {
            name: "go-account-a".to_string(),
            api_key: "sk-FAKE-BANDICOT-EXAMPLE-KEY".to_string(),
            base_url: None,
            auth_scheme: AuthScheme::Bearer,
        },
        ProviderEntry {
            name: "zen-account-b".to_string(),
            api_key: "sk-FAKE-BANDICOT-EXAMPLE-KEY".to_string(),
            base_url: None,
            auth_scheme: AuthScheme::Bearer,
        },
    ];

    let config = FallbackConfig {
        providers,
        recovery_interval_secs: 300, // 5 minutes
        max_failures: 3,
    };

    let provider = FallbackApiKeyProvider::with_config(config);
    let wrapper = FallbackWrapper::new(provider);

    println!(
        "Created fallback provider with {} providers",
        wrapper.provider().provider_count()
    );
    println!("Current provider: {:?}", wrapper.current_provider_name());
}

/// Example: Create from environment variable.
fn example_from_env() {
    if let Some(wrapper) = FallbackWrapper::from_env() {
        println!("Loaded fallback provider from environment");
        println!("Current provider: {:?}", wrapper.current_provider_name());
        println!("Current API key: {:?}", wrapper.current_api_key());
    } else {
        println!("No fallback provider configured");
    }
}

/// Example: Record success/failure and track provider health.
fn example_health_tracking() {
    let providers = vec![
        ProviderEntry {
            name: "primary".to_string(),
            api_key: "sk-primary".to_string(),
            base_url: None,
            auth_scheme: AuthScheme::Bearer,
        },
        ProviderEntry {
            name: "fallback".to_string(),
            api_key: "sk-fallback".to_string(),
            base_url: None,
            auth_scheme: AuthScheme::Bearer,
        },
    ];

    let wrapper = FallbackWrapper::new(FallbackApiKeyProvider::new(providers));

    // Simulate successful requests
    wrapper.record_success();
    wrapper.record_success();
    println!("After 2 successes: {:?}", wrapper.health_status());

    // Simulate rate limit failure - should switch to fallback
    wrapper.record_failure(true, false);
    println!(
        "After rate limit: current provider = {:?}",
        wrapper.current_provider_name()
    );

    // Simulate auth error - should switch back to primary (after recovery)
    wrapper.record_failure(false, true);
    println!(
        "After auth error: current provider = {:?}",
        wrapper.current_provider_name()
    );
}

/// Example: Create from JSON configuration.
fn example_from_json() {
    let json = r#"{
        "providers": [
            {"name": "openai", "api_key": "sk-openai-key", "base_url": "https://api.openai.com/v1"},
            {"name": "anthropic", "api_key": "sk-ant-key", "base_url": "https://api.anthropic.com"}
        ],
        "recovery_interval_secs": 600,
        "max_failures": 5
    }"#;

    match FallbackWrapper::from_json(json) {
        Ok(wrapper) => {
            println!("Created fallback provider from JSON");
            println!("Current provider: {:?}", wrapper.current_provider_name());
        }
        Err(e) => {
            eprintln!("Failed to parse JSON: {}", e);
        }
    }
}

fn main() {
    println!("=== API Key Fallback Provider Examples ===\n");

    println!("1. Create fallback provider:");
    example_create_fallback_provider();
    println!();

    println!("2. From environment:");
    example_from_env();
    println!();

    println!("3. Health tracking:");
    example_health_tracking();
    println!();

    println!("4. From JSON:");
    example_from_json();
}
