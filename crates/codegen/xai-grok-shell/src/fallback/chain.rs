//! Secret-free `[fallback]` configuration.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

/// Top-level `[fallback]` table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FallbackConfig {
    /// Master switch. Default off so single-key installs are unchanged.
    pub enabled: bool,
    /// Prefer the last successful hop for this many seconds. `0` = always
    /// start at the head of the chain.
    pub sticky_ttl_secs: u64,
    /// Maximum hops attempted per sampling request (including the primary).
    pub max_hops: u32,
    /// Emit TUI/status notifications when a hop is skipped or selected.
    pub notify: bool,
    /// When true, exhausted 5xx after transport retries also advances the
    /// chain. Default false (may be a global outage).
    pub failover_on_server_error: bool,
    /// Ordered account/provider hops.
    #[serde(default)]
    pub chain: Vec<FallbackChainHop>,
    /// Map selected catalog model id → per-provider catalog id.
    /// Key `"default"` is used when the session model has no entry.
    #[serde(default)]
    pub map: IndexMap<String, IndexMap<String, FallbackMapEntry>>,
}

impl Default for FallbackConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sticky_ttl_secs: 900,
            max_hops: 6,
            notify: true,
            failover_on_server_error: false,
            chain: Vec::new(),
            map: IndexMap::new(),
        }
    }
}

impl FallbackConfig {
    pub fn is_active(&self) -> bool {
        self.enabled && !self.chain.is_empty()
    }
}

/// One hop in `[[fallback.chain]]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FallbackChainHop {
    /// Stable id for sticky/cooldown state (e.g. `opencode-zen-1`).
    pub id: String,
    /// Logical provider used for model mapping.
    pub provider: FallbackProvider,
    /// Optional account label (Keychain account / documentation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account: Option<String>,
    /// Environment variable holding this hop's credential.
    pub env_key: String,
    /// Optional explicit catalog model id override for this hop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    /// Optional cost tier for UX (`subscription`, `metered`, `local`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_tier: Option<String>,
}

/// Known provider roles for mapping session models onto concrete catalog ids.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum FallbackProvider {
    OpencodeZen,
    OpencodeGo,
    OpenaiPlatform,
    OpenaiCodexPlan,
    AnthropicMessages,
    Ollama,
    Apple,
    /// Escape hatch for user-defined routes.
    #[serde(other)]
    Other,
}

impl FallbackProvider {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::OpencodeZen => "opencode_zen",
            Self::OpencodeGo => "opencode_go",
            Self::OpenaiPlatform => "openai_platform",
            Self::OpenaiCodexPlan => "openai_codex_plan",
            Self::AnthropicMessages => "anthropic_messages",
            Self::Ollama => "ollama",
            Self::Apple => "apple",
            Self::Other => "other",
        }
    }
}

/// Per-provider mapping under `[fallback.map.<session_model>]`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct FallbackMapEntry {
    /// Catalog key in `[model.*]` (preferred).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    /// Wire model id if catalog lookup is not used.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

/// Parse `[fallback]` from a raw TOML root value.
pub fn parse_fallback_config(root: &toml::Value) -> FallbackConfig {
    root.get("fallback")
        .cloned()
        .and_then(|v| v.try_into().ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_inactive() {
        let cfg = FallbackConfig::default();
        assert!(!cfg.is_active());
    }

    #[test]
    fn parse_chain() {
        let raw: toml::Value = toml::from_str(
            r#"
[fallback]
enabled = true
max_hops = 4

[[fallback.chain]]
id = "zen-1"
provider = "opencode_zen"
env_key = "OPENCODE_ZEN_API_KEY"

[[fallback.chain]]
id = "go-1"
provider = "opencode_go"
env_key = "OPENCODE_GO_API_KEY"
catalog = "go-kimi-k3"

[fallback.map.default]
opencode_go = { catalog = "go-kimi-k3" }
openai_platform = { catalog = "openai-latest" }
"#,
        )
        .unwrap();
        let cfg = parse_fallback_config(&raw);
        assert!(cfg.is_active());
        assert_eq!(cfg.chain.len(), 2);
        assert_eq!(cfg.chain[0].provider, FallbackProvider::OpencodeZen);
        assert_eq!(
            cfg.map["default"]["opencode_go"].catalog.as_deref(),
            Some("go-kimi-k3")
        );
    }
}
