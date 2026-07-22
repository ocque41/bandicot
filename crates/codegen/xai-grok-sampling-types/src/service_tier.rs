// Added in 2026 by the ocque41 OpenAI-support fork; see FORK-NOTICE.md.
//! Service-tier preference and resolution types.
//!
//! These are pure data contracts. Provider-specific request wiring lives in
//! the sampler crate.

use serde::{Deserialize, Serialize};

pub const OPENAI_PRIORITY_SERVICE_TIER: &str = "priority";
pub const OPENAI_RESPONSES_MULTI_AGENT_BETA: &str = "responses_multi_agent=v1";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTierPreference {
    #[default]
    Inherit,
    Standard,
    Fast,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EffectiveServiceTier {
    #[default]
    Standard,
    Priority,
    Other(String),
}

impl EffectiveServiceTier {
    pub fn responses_wire_value(&self) -> Option<&str> {
        match self {
            Self::Standard => None,
            Self::Priority => Some(OPENAI_PRIORITY_SERVICE_TIER),
            Self::Other(value) => Some(value.as_str()),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ServiceTierSource {
    #[default]
    Default,
    Config,
    Session,
    ModelCatalog,
    ProviderCapability,
    BuiltIn,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ServiceTierCapabilities {
    #[serde(default)]
    pub priority: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_service_tier: Option<EffectiveServiceTier>,
}

impl Default for ServiceTierCapabilities {
    fn default() -> Self {
        Self {
            priority: false,
            default_service_tier: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResolvedServiceTier {
    pub requested: ServiceTierPreference,
    pub effective: EffectiveServiceTier,
    pub source: ServiceTierSource,
    pub supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl Default for ResolvedServiceTier {
    fn default() -> Self {
        Self::standard(ServiceTierPreference::Inherit, ServiceTierSource::Default)
    }
}

impl ResolvedServiceTier {
    pub fn standard(requested: ServiceTierPreference, source: ServiceTierSource) -> Self {
        Self {
            requested,
            effective: EffectiveServiceTier::Standard,
            source,
            supported: true,
            reason: None,
        }
    }

    pub fn fast(source: ServiceTierSource) -> Self {
        Self {
            requested: ServiceTierPreference::Fast,
            effective: EffectiveServiceTier::Priority,
            source,
            supported: true,
            reason: None,
        }
    }

    pub fn unsupported_fast(source: ServiceTierSource, reason: impl Into<String>) -> Self {
        Self {
            requested: ServiceTierPreference::Fast,
            effective: EffectiveServiceTier::Standard,
            source,
            supported: false,
            reason: Some(reason.into()),
        }
    }

    pub fn responses_wire_value(&self) -> Option<&str> {
        self.effective.responses_wire_value()
    }
}

pub fn resolve_service_tier(
    requested: ServiceTierPreference,
    capabilities: &ServiceTierCapabilities,
    source: ServiceTierSource,
) -> ResolvedServiceTier {
    match requested {
        ServiceTierPreference::Standard => {
            ResolvedServiceTier::standard(ServiceTierPreference::Standard, source)
        }
        ServiceTierPreference::Fast if capabilities.priority => ResolvedServiceTier::fast(source),
        ServiceTierPreference::Fast => ResolvedServiceTier::unsupported_fast(
            source,
            "priority service tier is not supported by the selected provider/model",
        ),
        ServiceTierPreference::Inherit => match &capabilities.default_service_tier {
            Some(EffectiveServiceTier::Priority) if capabilities.priority => ResolvedServiceTier {
                requested,
                effective: EffectiveServiceTier::Priority,
                source,
                supported: true,
                reason: None,
            },
            Some(EffectiveServiceTier::Other(value)) => ResolvedServiceTier {
                requested,
                effective: EffectiveServiceTier::Other(value.clone()),
                source,
                supported: true,
                reason: None,
            },
            _ => ResolvedServiceTier::standard(requested, source),
        },
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HostedMultiAgentCapability {
    #[serde(default)]
    pub supported: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_subagents: Option<u32>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct HostedMultiAgentConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrent_subagents: Option<u32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_supported_resolves_to_priority_wire_value() {
        let resolved = resolve_service_tier(
            ServiceTierPreference::Fast,
            &ServiceTierCapabilities {
                priority: true,
                default_service_tier: None,
            },
            ServiceTierSource::Session,
        );

        assert_eq!(resolved.requested, ServiceTierPreference::Fast);
        assert_eq!(resolved.effective, EffectiveServiceTier::Priority);
        assert_eq!(
            resolved.responses_wire_value(),
            Some(OPENAI_PRIORITY_SERVICE_TIER)
        );
        assert!(resolved.supported);
    }

    #[test]
    fn explicit_standard_is_not_overridden_by_catalog_priority_default() {
        let resolved = resolve_service_tier(
            ServiceTierPreference::Standard,
            &ServiceTierCapabilities {
                priority: true,
                default_service_tier: Some(EffectiveServiceTier::Priority),
            },
            ServiceTierSource::Config,
        );

        assert_eq!(resolved.requested, ServiceTierPreference::Standard);
        assert_eq!(resolved.effective, EffectiveServiceTier::Standard);
        assert_eq!(resolved.responses_wire_value(), None);
        assert!(resolved.supported);
    }

    #[test]
    fn unsupported_fast_preserves_requested_intent() {
        let resolved = resolve_service_tier(
            ServiceTierPreference::Fast,
            &ServiceTierCapabilities::default(),
            ServiceTierSource::Session,
        );

        assert_eq!(resolved.requested, ServiceTierPreference::Fast);
        assert_eq!(resolved.effective, EffectiveServiceTier::Standard);
        assert_eq!(resolved.responses_wire_value(), None);
        assert!(!resolved.supported);
        assert!(resolved.reason.is_some());
    }

    #[test]
    fn inherit_can_use_supported_catalog_priority_default() {
        let resolved = resolve_service_tier(
            ServiceTierPreference::Inherit,
            &ServiceTierCapabilities {
                priority: true,
                default_service_tier: Some(EffectiveServiceTier::Priority),
            },
            ServiceTierSource::ModelCatalog,
        );

        assert_eq!(resolved.requested, ServiceTierPreference::Inherit);
        assert_eq!(resolved.effective, EffectiveServiceTier::Priority);
        assert_eq!(
            resolved.responses_wire_value(),
            Some(OPENAI_PRIORITY_SERVICE_TIER)
        );
        assert!(resolved.supported);
    }
}
