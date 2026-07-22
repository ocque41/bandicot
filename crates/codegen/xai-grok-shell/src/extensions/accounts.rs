//! Typed ACP operations for provider accounts, routes, and model preferences.

use agent_client_protocol as acp;
use serde::Deserialize;

use super::{ExtResult, parse_params, to_raw_response};
use crate::accounts::AccountManager;
use crate::accounts::storage::{AccountAuth, FallbackRoute, ProviderId};

pub mod methods {
    pub const SNAPSHOT: &str = "x.ai/accounts/snapshot";
    pub const ADD: &str = "x.ai/accounts/add";
    pub const EDIT: &str = "x.ai/accounts/edit";
    pub const REMOVE: &str = "x.ai/accounts/remove";
    pub const ENABLE: &str = "x.ai/accounts/enable";
    pub const DISABLE: &str = "x.ai/accounts/disable";
    pub const VALIDATE: &str = "x.ai/accounts/validate";
    pub const REFRESH_MODELS: &str = "x.ai/accounts/refresh_models";
    pub const UPDATE_ALLOWLIST: &str = "x.ai/accounts/update_allowlist";
    pub const GET_FALLBACK: &str = "x.ai/accounts/fallback/get";
    pub const REPLACE_FALLBACK: &str = "x.ai/accounts/fallback/replace";
    pub const GET_PREFERENCES: &str = "x.ai/accounts/preferences/get";
    pub const UPDATE_PREFERENCES: &str = "x.ai/accounts/preferences/update";
}

fn internal(error: anyhow::Error) -> acp::Error {
    acp::Error::internal_error().data(error.to_string())
}

fn auth(provider: &ProviderId, auth_type: Option<&str>, secret: Option<String>) -> AccountAuth {
    match auth_type {
        Some("none") => AccountAuth::None,
        Some("external_proxy") => AccountAuth::ExternalProxy {
            client_token: secret.unwrap_or_default(),
        },
        _ if matches!(provider.as_str(), ProviderId::OLLAMA | ProviderId::APPLE) => {
            AccountAuth::None
        }
        _ if provider.as_str() == ProviderId::OPENAI_CODEX_PLAN => AccountAuth::ExternalProxy {
            client_token: secret.unwrap_or_default(),
        },
        _ => AccountAuth::ApiKey {
            secret: secret.unwrap_or_default(),
        },
    }
}

#[tracing::instrument(skip_all, fields(method = %args.method))]
pub async fn handle(args: &acp::ExtRequest) -> ExtResult {
    match args.method.as_ref() {
        methods::SNAPSHOT => {
            let manager = AccountManager::load().map_err(internal)?;
            to_raw_response(&manager.snapshot())
        }
        methods::ADD => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Params {
                label: String,
                provider_id: String,
                #[serde(default)]
                secret: Option<String>,
                #[serde(default)]
                auth_type: Option<String>,
                #[serde(default)]
                base_url: Option<String>,
                #[serde(default)]
                model_allowlist: Vec<String>,
            }
            let params: Params = parse_params(args)?;
            let provider = ProviderId::parse(&params.provider_id);
            let account_auth = auth(&provider, params.auth_type.as_deref(), params.secret);
            let secret = account_auth.credential().unwrap_or_default().to_owned();
            let mut manager = AccountManager::load().map_err(internal)?;
            let entry = manager
                .add(params.label, provider, secret, params.base_url, None)
                .map_err(internal)?;
            manager
                .edit(&entry.id, None, None, Some(account_auth), None, None)
                .map_err(internal)?;
            if !params.model_allowlist.is_empty() {
                manager
                    .update_models(&entry.id, None, Some(params.model_allowlist))
                    .map_err(internal)?;
            }
            to_raw_response(&manager.snapshot())
        }
        methods::EDIT => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Params {
                account_id: String,
                #[serde(default)]
                label: Option<String>,
                #[serde(default)]
                provider_id: Option<String>,
                #[serde(default)]
                auth_type: Option<String>,
                #[serde(default)]
                secret: Option<String>,
                #[serde(default)]
                base_url: Option<Option<String>>,
                #[serde(default)]
                enabled: Option<bool>,
            }
            let params: Params = parse_params(args)?;
            let mut manager = AccountManager::load().map_err(internal)?;
            let current = manager
                .get_account_by_id(&params.account_id)
                .cloned()
                .ok_or_else(|| acp::Error::invalid_params().data("account not found"))?;
            let provider = params.provider_id.as_deref().map(ProviderId::parse);
            let next_provider = provider.as_ref().unwrap_or(&current.provider);
            let next_auth = (params.auth_type.is_some() || params.secret.is_some())
                .then(|| auth(next_provider, params.auth_type.as_deref(), params.secret));
            manager
                .edit(
                    &params.account_id,
                    params.label,
                    provider,
                    next_auth,
                    params.base_url,
                    params.enabled,
                )
                .map_err(internal)?;
            to_raw_response(&manager.snapshot())
        }
        methods::REMOVE | methods::ENABLE | methods::DISABLE => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Params {
                account_id: String,
            }
            let params: Params = parse_params(args)?;
            let mut manager = AccountManager::load().map_err(internal)?;
            match args.method.as_ref() {
                methods::REMOVE => {
                    manager.remove_by_id(&params.account_id).map_err(internal)?;
                }
                methods::ENABLE => {
                    manager
                        .edit(&params.account_id, None, None, None, None, Some(true))
                        .map_err(internal)?;
                }
                methods::DISABLE => {
                    manager
                        .edit(&params.account_id, None, None, None, None, Some(false))
                        .map_err(internal)?;
                }
                _ => unreachable!(),
            }
            to_raw_response(&manager.snapshot())
        }
        methods::VALIDATE | methods::REFRESH_MODELS => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Params {
                account_id: String,
            }
            let params: Params = parse_params(args)?;
            let mut manager = AccountManager::load().map_err(internal)?;
            let account = manager
                .get_account_by_id(&params.account_id)
                .cloned()
                .ok_or_else(|| acp::Error::invalid_params().data("account not found"))?;
            let models = crate::accounts::providers::discover_models(&account)
                .await
                .map_err(internal)?;
            manager
                .update_models(&params.account_id, Some(models), None)
                .map_err(internal)?;
            to_raw_response(&manager.snapshot())
        }
        methods::UPDATE_ALLOWLIST => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Params {
                account_id: String,
                model_allowlist: Vec<String>,
            }
            let params: Params = parse_params(args)?;
            let mut manager = AccountManager::load().map_err(internal)?;
            manager
                .update_models(&params.account_id, None, Some(params.model_allowlist))
                .map_err(internal)?;
            to_raw_response(&manager.snapshot())
        }
        methods::GET_FALLBACK => {
            let manager = AccountManager::load().map_err(internal)?;
            to_raw_response(&manager.snapshot().fallback_chain)
        }
        methods::REPLACE_FALLBACK => {
            #[derive(Deserialize)]
            struct Params {
                chain: Vec<FallbackRoute>,
            }
            let params: Params = parse_params(args)?;
            let mut manager = AccountManager::load().map_err(internal)?;
            manager.set_fallback_chain(params.chain).map_err(internal)?;
            to_raw_response(&manager.snapshot())
        }
        methods::GET_PREFERENCES => {
            let manager = AccountManager::load().map_err(internal)?;
            to_raw_response(&manager.snapshot().model_preferences)
        }
        methods::UPDATE_PREFERENCES => {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Params {
                route_id: String,
                #[serde(default)]
                favorite: Option<bool>,
                #[serde(default)]
                recent: bool,
            }
            let params: Params = parse_params(args)?;
            let mut manager = AccountManager::load().map_err(internal)?;
            let preferences = manager
                .update_preferences(&params.route_id, params.favorite, params.recent)
                .map_err(internal)?;
            to_raw_response(&preferences)
        }
        _ => Err(acp::Error::method_not_found()),
    }
}
