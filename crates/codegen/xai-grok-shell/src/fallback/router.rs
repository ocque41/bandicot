//! Plan ordered hops for a sampling request.

use crate::agent::config::{EnvKeys, ModelEntry};

use super::chain::{FallbackChainHop, FallbackConfig, FallbackProvider};
use super::classify::FailoverReason;
use super::state::FallbackState;

/// One planned attempt in a fallback walk.
#[derive(Debug, Clone)]
pub struct HopAttempt {
    pub hop: FallbackChainHop,
    pub catalog_id: String,
}

/// Planned hop sequence for one request.
#[derive(Debug, Clone)]
pub struct FallbackPlan {
    pub hops: Vec<HopAttempt>,
    /// True when the plan is just the current session model (fallback off).
    pub passthrough: bool,
}

/// Resolve the catalog model id for a hop given the session's selected model.
pub fn resolve_hop_catalog_id(
    hop: &FallbackChainHop,
    session_model_id: &str,
    cfg: &FallbackConfig,
) -> String {
    if let Some(catalog) = hop.catalog.as_ref().filter(|s| !s.is_empty()) {
        return catalog.clone();
    }
    let provider_key = hop.provider.as_str();
    for map_key in [session_model_id, "default"] {
        if let Some(provider_map) = cfg.map.get(map_key) {
            if let Some(entry) = provider_map.get(provider_key) {
                if let Some(catalog) = entry.catalog.as_ref().filter(|s| !s.is_empty()) {
                    return catalog.clone();
                }
                if let Some(model) = entry.model.as_ref().filter(|s| !s.is_empty()) {
                    return model.clone();
                }
            }
        }
    }
    // Built-in defaults when map is empty.
    default_catalog_for_provider(hop.provider, session_model_id)
}

fn default_catalog_for_provider(provider: FallbackProvider, session_model_id: &str) -> String {
    match provider {
        FallbackProvider::OpencodeZen => {
            if session_model_id.starts_with("zen-") {
                session_model_id.to_owned()
            } else {
                "zen-claude-sonnet-4-6".to_owned()
            }
        }
        FallbackProvider::OpencodeGo => {
            if session_model_id.starts_with("go-") {
                session_model_id.to_owned()
            } else {
                "go-kimi-k3".to_owned()
            }
        }
        FallbackProvider::OpenaiPlatform | FallbackProvider::OpenaiCodexPlan => {
            if session_model_id.starts_with("openai-") {
                session_model_id.to_owned()
            } else {
                "openai-latest".to_owned()
            }
        }
        FallbackProvider::AnthropicMessages => "claude-sonnet".to_owned(),
        FallbackProvider::Ollama => "ollama-gpt-oss-20b".to_owned(),
        FallbackProvider::Apple => "apple-on-device".to_owned(),
        FallbackProvider::Other => session_model_id.to_owned(),
    }
}

/// Build the ordered hop list for this request.
pub fn plan_hops(
    cfg: &FallbackConfig,
    state: &FallbackState,
    session_model_id: &str,
    models: &indexmap::IndexMap<String, ModelEntry>,
) -> FallbackPlan {
    if !cfg.is_active() {
        return FallbackPlan {
            hops: Vec::new(),
            passthrough: true,
        };
    }

    let max = cfg.max_hops.max(1) as usize;
    let mut ordered: Vec<&FallbackChainHop> = cfg.chain.iter().collect();

    // Sticky: rotate so sticky hop is first if still eligible.
    if let Some(sticky) = state.sticky_hop_id() {
        if let Some(pos) = ordered.iter().position(|h| h.id == sticky) {
            let sticky_hop = ordered.remove(pos);
            ordered.insert(0, sticky_hop);
        }
    }

    let mut hops = Vec::new();
    for hop in ordered {
        if hops.len() >= max {
            break;
        }
        if state.is_cooling_down(&hop.id) {
            continue;
        }
        if !credential_resolvable(&hop.env_key) {
            continue;
        }
        let catalog_id = resolve_hop_catalog_id(hop, session_model_id, cfg);
        // Prefer catalog presence; still include if missing so caller can
        // report capability/missing-model clearly.
        let _ = models.get(&catalog_id);
        hops.push(HopAttempt {
            hop: hop.clone(),
            catalog_id,
        });
    }

    FallbackPlan {
        hops,
        passthrough: false,
    }
}

fn credential_resolvable(env_key: &str) -> bool {
    EnvKeys::single(env_key)
        .resolve_value()
        .is_some_and(|v| !v.trim().is_empty())
}

/// Record a hop failure into state; returns the reason when failover should continue.
pub fn note_failure(
    state: &mut FallbackState,
    hop_id: &str,
    reason: FailoverReason,
    retry_after_secs: Option<u64>,
) {
    state.mark_exhausted(hop_id, reason, retry_after_secs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::config::{EnvKeys, ModelEntry, ModelInfo};
    use crate::fallback::chain::{FallbackChainHop, FallbackConfig, FallbackProvider};
    use indexmap::IndexMap;
    use xai_grok_test_support::env::EnvGuard;

    fn hop(id: &str, provider: FallbackProvider, env: &str) -> FallbackChainHop {
        FallbackChainHop {
            id: id.into(),
            provider,
            account: None,
            env_key: env.into(),
            catalog: None,
            cost_tier: None,
        }
    }

    #[test]
    #[serial_test::serial]
    fn plan_skips_missing_and_cooldown() {
        let _a = EnvGuard::set("FB_TEST_A", "key-a");
        let _b = EnvGuard::set("FB_TEST_B", "key-b");
        let mut cfg = FallbackConfig {
            enabled: true,
            max_hops: 6,
            ..FallbackConfig::default()
        };
        cfg.chain = vec![
            hop("a", FallbackProvider::OpencodeZen, "FB_TEST_A"),
            hop("b", FallbackProvider::OpencodeGo, "FB_TEST_B"),
            hop("c", FallbackProvider::OpenaiPlatform, "FB_TEST_MISSING"),
        ];
        let mut state = FallbackState::new();
        state.mark_exhausted("a", FailoverReason::RateLimited, Some(600));
        let models = IndexMap::<String, ModelEntry>::new();
        let plan = plan_hops(&cfg, &state, "openai-sol", &models);
        assert_eq!(plan.hops.len(), 1);
        assert_eq!(plan.hops[0].hop.id, "b");
        assert_eq!(plan.hops[0].catalog_id, "go-kimi-k3");
    }

    #[test]
    fn resolve_explicit_catalog() {
        let h = FallbackChainHop {
            id: "x".into(),
            provider: FallbackProvider::OpencodeGo,
            account: None,
            env_key: "K".into(),
            catalog: Some("go-glm-5".into()),
            cost_tier: None,
        };
        let cfg = FallbackConfig::default();
        assert_eq!(resolve_hop_catalog_id(&h, "openai-sol", &cfg), "go-glm-5");
    }
}
