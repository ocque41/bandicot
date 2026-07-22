# API Key Fallback Provider Implementation

## Summary

Implemented a complete API key fallback system for the bandicot project that automatically switches between API keys when one fails (rate limit, auth error, or circuit breaker open).

## Files Created

### 1. `crates/codegen/xai-grok-tools/src/types/fallback_provider.rs`
Core implementation of the fallback provider chain:
- `FallbackApiKeyProvider` - Main struct implementing `ApiKeyProvider` trait
- `ProviderEntry` - Individual provider configuration
- `FallbackConfig` - Configuration for the fallback chain
- `ProviderHealth` - Health tracking for each provider

### 2. `crates/codegen/xai-grok-tools/src/types/fallback_wrapper.rs`
Session-level wrapper for the fallback provider:
- `FallbackWrapper` - Thread-safe wrapper for session integration
- Success/failure recording
- Health status reporting

### 3. `crates/codegen/xai-grok-tools/examples/fallback_provider_example.rs`
Example showing how to use the fallback provider.

## Key Features

### 1. Automatic Provider Switching
When a provider fails with:
- HTTP 429 (Rate Limited)
- HTTP 401/403 (Auth errors)
- Circuit breaker open (consecutive failures)

The system automatically moves to the next provider in the chain.

### 2. Circuit Breaker Pattern
Each provider has its own health tracking:
- Tracks consecutive failures
- Opens circuit after `max_failures` (default: 3)
- Periodically attempts recovery after `recovery_interval_secs` (default: 300s)

### 3. Configuration Options
- Environment variable: `GROK_FALLBACK_KEYS`
- JSON configuration
- Programmatic configuration

### 4. Thread-Safe Design
- Atomic index for provider switching
- Mutex-protected health state
- Clone-safe for concurrent access

## Configuration Example

```bash
export GROK_FALLBACK_KEYS='{
  "providers": [
    {"name": "zen-a", "api_key": "sk-FAKE-BANDICOT-EXAMPLE-KEY-A"},
    {"name": "go-a", "api_key": "sk-FAKE-BANDICOT-EXAMPLE-KEY-B"},
    {"name": "zen-b", "api_key": "sk-FAKE-BANDICOT-EXAMPLE-KEY-C"}
  ],
  "recovery_interval_secs": 300,
  "max_failures": 3
}'
```

## Usage in Session

```rust
use xai_grok_tools::types::fallback_wrapper::FallbackWrapper;

// Create fallback wrapper from environment
let fallback = FallbackWrapper::from_env();

// Use in session initialization
if let Some(wrapper) = fallback {
    // Get current API key
    let api_key = wrapper.current_api_key();
    
    // After successful request
    wrapper.record_success();
    
    // After failed request (rate limit or auth error)
    wrapper.record_failure(true, false); // is_rate_limit, is_auth_error
}
```

## Integration Points

### 1. With Existing `ApiKeyProvider` Trait
The `FallbackApiKeyProvider` implements the existing `ApiKeyProvider` trait, making it a drop-in replacement.

### 2. With Session Layer
The `FallbackWrapper` can be used in the session layer to:
- Resolve API keys for requests
- Record success/failure events
- Track provider health

### 3. With Circuit Breaker
Each provider has its own circuit breaker that:
- Opens after consecutive failures
- Periodically attempts recovery
- Prevents hammering failing providers

## Test Coverage

All tests pass:
- 48 unit tests in `xai-grok-tools`
- 2 doc tests for the new modules
- Example runs successfully

## Next Steps for Full Integration

1. **Session Layer Integration**: Modify `SessionActor` to use `FallbackWrapper`
2. **Sampler Integration**: Add fallback logic to `apply_retry_decision` in `request_task.rs`
3. **Telemetry**: Add metrics for fallback events
4. **Configuration UI**: Add configuration options to the CLI/config file
