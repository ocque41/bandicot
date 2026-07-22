use serde::{Deserialize, Serialize};
use xai_grok_sampling_types::{ApiBackend, ReasoningEffort as ProviderReasoningEffort};

use crate::agent::{config::ModelInfo, models::ModelsManager};

use super::types::{ReasoningEffort, ServiceTierPreference};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCandidate {
    pub model_slug: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSelectorSpec {
    pub name: String,
    pub candidates: Vec<ModelCandidate>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_api_backend: Option<ApiBackend>,
    #[serde(default)]
    pub require_image_input: bool,
    #[serde(default)]
    pub require_structured_output: bool,
    #[serde(default)]
    pub require_tools: bool,
    #[serde(default)]
    pub required_service_tier: ServiceTierPreference,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub required_reasoning_effort: Option<ReasoningEffort>,
    #[serde(default)]
    pub allow_hidden: bool,
    #[serde(default)]
    pub allow_inherit: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelResolution {
    pub requested_selector: String,
    pub selected_model: String,
    pub provider: String,
    pub reasoning_effort: ReasoningEffort,
    pub service_tier: ServiceTierPreference,
    pub source: ModelResolutionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelResolutionSource {
    LiveCatalog,
    Inherited,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelResolutionError {
    UnknownSelector(String),
    NoCandidate {
        selector: String,
        reasons: Vec<String>,
    },
}

pub fn builtin_selector(name: &str) -> Option<ModelSelectorSpec> {
    let candidate = |model_slug: &str| ModelCandidate {
        model_slug: model_slug.to_string(),
        provider: None,
    };
    match name {
        "worker-light" | "luna" => Some(ModelSelectorSpec {
            name: name.to_string(),
            candidates: vec![candidate("gpt-5.6-luna")],
            required_reasoning_effort: Some(ReasoningEffort::Low),
            ..selector_defaults()
        }),
        "reducer-balanced" | "terra" => Some(ModelSelectorSpec {
            name: name.to_string(),
            candidates: vec![candidate("gpt-5.6-terra")],
            ..selector_defaults()
        }),
        "critical-verifier" | "sol" => Some(ModelSelectorSpec {
            name: name.to_string(),
            candidates: vec![candidate("gpt-5.6-sol"), candidate("gpt-5.6")],
            ..selector_defaults()
        }),
        "inherit" => Some(ModelSelectorSpec {
            name: name.to_string(),
            candidates: Vec::new(),
            allow_inherit: true,
            ..selector_defaults()
        }),
        explicit if explicit.starts_with("gpt-") => Some(ModelSelectorSpec {
            name: explicit.to_string(),
            candidates: vec![candidate(explicit)],
            ..selector_defaults()
        }),
        _ => None,
    }
}

fn selector_defaults() -> ModelSelectorSpec {
    ModelSelectorSpec {
        name: String::new(),
        candidates: Vec::new(),
        required_api_backend: None,
        require_image_input: false,
        require_structured_output: false,
        require_tools: false,
        required_service_tier: ServiceTierPreference::Inherit,
        required_reasoning_effort: None,
        allow_hidden: false,
        allow_inherit: false,
    }
}

pub fn resolve_live_selector(
    manager: &ModelsManager,
    selector: &ModelSelectorSpec,
    requested_effort: ReasoningEffort,
    requested_tier: ServiceTierPreference,
) -> Result<ModelResolution, ModelResolutionError> {
    resolve_selector(
        manager.agent_graph_catalog().iter(),
        selector,
        requested_effort,
        requested_tier,
    )
}

pub fn resolve_selector<'a>(
    catalog: impl IntoIterator<Item = &'a ModelInfo>,
    selector: &ModelSelectorSpec,
    requested_effort: ReasoningEffort,
    requested_tier: ServiceTierPreference,
) -> Result<ModelResolution, ModelResolutionError> {
    if selector.allow_inherit && selector.candidates.is_empty() {
        return Ok(ModelResolution {
            requested_selector: selector.name.clone(),
            selected_model: "inherit".to_string(),
            provider: "inherit".to_string(),
            reasoning_effort: requested_effort,
            service_tier: requested_tier,
            source: ModelResolutionSource::Inherited,
        });
    }
    let catalog = catalog.into_iter().collect::<Vec<_>>();
    let mut reasons = Vec::new();
    for candidate in &selector.candidates {
        let Some(model) = catalog.iter().copied().find(|model| {
            model.model == candidate.model_slug
                || model.id.as_deref() == Some(candidate.model_slug.as_str())
        }) else {
            reasons.push(format!(
                "{} is absent from the live catalog",
                candidate.model_slug
            ));
            continue;
        };
        if model.hidden && !selector.allow_hidden {
            reasons.push(format!("{} is hidden", candidate.model_slug));
            continue;
        }
        let provider = safe_provider_id(&model.base_url);
        if candidate
            .provider
            .as_deref()
            .is_some_and(|wanted| wanted != provider)
        {
            reasons.push(format!(
                "{} is on provider {provider}",
                candidate.model_slug
            ));
            continue;
        }
        if selector
            .required_api_backend
            .as_ref()
            .is_some_and(|backend| backend != &model.api_backend)
        {
            reasons.push(format!(
                "{} uses a different API backend",
                candidate.model_slug
            ));
            continue;
        }
        if selector.require_image_input && !model.capabilities.image_input {
            reasons.push(format!("{} lacks image input", candidate.model_slug));
            continue;
        }
        if selector.require_tools && !model.capabilities.tools {
            reasons.push(format!("{} lacks tool support", candidate.model_slug));
            continue;
        }
        if selector.require_structured_output && model.api_backend != ApiBackend::Responses {
            reasons.push(format!(
                "{} lacks required structured output",
                candidate.model_slug
            ));
            continue;
        }
        if matches!(selector.required_service_tier, ServiceTierPreference::Fast)
            && !model.capabilities.service_tiers.priority
        {
            reasons.push(format!(
                "{} lacks priority service tier",
                candidate.model_slug
            ));
            continue;
        }
        let effort = selector
            .required_reasoning_effort
            .unwrap_or(requested_effort);
        if !supports_effort(model, effort) {
            reasons.push(format!(
                "{} does not support {effort:?} effort",
                candidate.model_slug
            ));
            continue;
        }
        return Ok(ModelResolution {
            requested_selector: selector.name.clone(),
            selected_model: model.model.clone(),
            provider: provider.to_string(),
            reasoning_effort: effort,
            service_tier: requested_tier,
            source: ModelResolutionSource::LiveCatalog,
        });
    }
    Err(ModelResolutionError::NoCandidate {
        selector: selector.name.clone(),
        reasons,
    })
}

fn supports_effort(model: &ModelInfo, effort: ReasoningEffort) -> bool {
    if effort == ReasoningEffort::None {
        return true;
    }
    if !model.supports_reasoning_effort && model.reasoning_efforts.is_empty() {
        return false;
    }
    let provider_effort = match effort {
        ReasoningEffort::None => ProviderReasoningEffort::None,
        ReasoningEffort::Minimal => ProviderReasoningEffort::Minimal,
        ReasoningEffort::Low => ProviderReasoningEffort::Low,
        ReasoningEffort::Medium => ProviderReasoningEffort::Medium,
        ReasoningEffort::High => ProviderReasoningEffort::High,
        ReasoningEffort::Xhigh => ProviderReasoningEffort::Xhigh,
        ReasoningEffort::Max => ProviderReasoningEffort::Max,
    };
    model.reasoning_efforts.is_empty()
        || model
            .reasoning_efforts
            .iter()
            .any(|option| option.value == provider_effort)
}

fn safe_provider_id(base_url: &str) -> &str {
    let authority = base_url
        .split_once("://")
        .map_or(base_url, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or("unknown");
    authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(slug: &str) -> ModelInfo {
        let mut model = ModelInfo::fallback(slug);
        model.base_url = "https://provider.example/v1".to_string();
        model.supports_reasoning_effort = true;
        model
    }

    #[test]
    fn first_available_candidate_and_fallback_are_ordered() {
        let selector = builtin_selector("critical-verifier").unwrap();
        let fallback = model("gpt-5.6");
        assert_eq!(
            resolve_selector(
                [&fallback],
                &selector,
                ReasoningEffort::High,
                ServiceTierPreference::Standard
            )
            .unwrap()
            .selected_model,
            "gpt-5.6"
        );
        let primary = model("gpt-5.6-sol");
        assert_eq!(
            resolve_selector(
                [&fallback, &primary],
                &selector,
                ReasoningEffort::High,
                ServiceTierPreference::Standard
            )
            .unwrap()
            .selected_model,
            "gpt-5.6-sol"
        );
    }

    #[test]
    fn exact_luna_selector_has_no_silent_fallback() {
        let selector = builtin_selector("worker-light").unwrap();
        let terra = model("gpt-5.6-terra");
        assert!(
            resolve_selector(
                [&terra],
                &selector,
                ReasoningEffort::Low,
                ServiceTierPreference::Standard
            )
            .is_err()
        );
    }

    #[test]
    fn capability_provider_hidden_and_effort_constraints_fail_closed() {
        let mut candidate = model("gpt-test");
        candidate.hidden = true;
        candidate.supports_reasoning_effort = false;
        candidate.api_backend = ApiBackend::ChatCompletions;
        let mut selector = builtin_selector("gpt-test").unwrap();
        selector.require_structured_output = true;
        selector.required_reasoning_effort = Some(ReasoningEffort::Max);
        assert!(
            resolve_selector(
                [&candidate],
                &selector,
                ReasoningEffort::Max,
                ServiceTierPreference::Fast
            )
            .is_err()
        );
    }
}
