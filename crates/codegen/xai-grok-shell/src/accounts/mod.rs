//! Account, provider-route, model-preference, and fallback management.

pub mod providers;
pub mod storage;

use anyhow::Result;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use storage::{
    AccountEntry, AccountsFile, FallbackRoute, ModelPreferences, ProviderId, add_account,
    disable_account, edit_account, enable_account, load_accounts, remove_account,
    remove_account_by_id, reorder_account, replace_fallback_chain, update_account_models,
    update_model_preferences,
};

/// Secret-free account data returned to ACP clients and rendered by the pager.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AccountSummary {
    pub id: String,
    pub name: String,
    pub provider_id: String,
    pub provider_name: String,
    pub enabled: bool,
    pub auth_type: String,
    pub base_url: Option<String>,
    pub model_ids: Vec<String>,
    pub discovered_models: Vec<storage::DiscoveredModel>,
    pub models_refreshed_at: Option<String>,
}

impl From<&AccountEntry> for AccountSummary {
    fn from(value: &AccountEntry) -> Self {
        let auth_type = match value.auth {
            storage::AccountAuth::ApiKey { .. } => "api_key",
            storage::AccountAuth::ExternalProxy { .. } => "external_proxy",
            storage::AccountAuth::None => "none",
        };
        Self {
            id: value.id.clone(),
            name: value.name.clone(),
            provider_id: value.provider.as_str().to_owned(),
            provider_name: value.provider.display_name().to_owned(),
            enabled: value.enabled,
            auth_type: auth_type.to_owned(),
            base_url: value.base_url.clone(),
            model_ids: value.model_allowlist.clone(),
            discovered_models: value.discovered_models.clone(),
            models_refreshed_at: value.models_refreshed_at.clone(),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct ProviderSummary {
    pub id: String,
    pub name: String,
    pub default_base_url: Option<String>,
    pub authentication_types: Vec<String>,
}

fn provider_summaries(accounts: &[AccountEntry]) -> Vec<ProviderSummary> {
    let mut providers = vec![
        ProviderId::parse("zen"),
        ProviderId::parse("go"),
        ProviderId::parse("openai"),
        ProviderId::parse("anthropic"),
        ProviderId::parse("codex"),
        ProviderId::parse("ollama"),
    ];
    providers.extend(accounts.iter().map(|account| account.provider.clone()));
    providers.sort();
    providers.dedup();
    providers
        .into_iter()
        .map(|provider| {
            let keyless = matches!(provider.as_str(), ProviderId::OLLAMA | ProviderId::APPLE);
            ProviderSummary {
                id: provider.as_str().to_owned(),
                name: provider.display_name().to_owned(),
                default_base_url: provider.default_base_url().map(str::to_owned),
                authentication_types: if keyless {
                    vec!["none".to_owned()]
                } else if provider.as_str() == ProviderId::OPENAI_CODEX_PLAN {
                    vec!["external_proxy".to_owned()]
                } else {
                    vec!["api_key".to_owned()]
                },
            }
        })
        .collect()
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct AccountSnapshot {
    pub version: u32,
    pub generation: u64,
    pub accounts: Vec<AccountSummary>,
    pub providers: Vec<ProviderSummary>,
    pub fallback_chain: Vec<FallbackRoute>,
    pub model_preferences: ModelPreferences,
    pub route_health: Vec<RouteHealth>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct RouteHealth {
    pub route_id: String,
    pub state: String,
    pub reason: String,
    pub retry_after_secs: u64,
}

#[derive(Debug, Clone)]
struct RouteCooldown {
    reason: String,
    until: Instant,
}

static ROUTE_COOLDOWNS: LazyLock<Mutex<HashMap<String, RouteCooldown>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub fn mark_route_cooldown(route_id: &str, reason: &str, retry_after_secs: Option<u64>) {
    let fallback = if reason == "quota_exhausted" { 900 } else { 60 };
    let duration = retry_after_secs.unwrap_or(fallback).max(1);
    ROUTE_COOLDOWNS.lock().unwrap().insert(
        route_id.to_owned(),
        RouteCooldown {
            reason: reason.to_owned(),
            until: Instant::now() + Duration::from_secs(duration),
        },
    );
}

pub fn route_is_available(route_id: &str) -> bool {
    let now = Instant::now();
    let mut health = ROUTE_COOLDOWNS.lock().unwrap();
    health.retain(|_, cooldown| cooldown.until > now);
    !health.contains_key(route_id)
}

fn route_health() -> Vec<RouteHealth> {
    let now = Instant::now();
    let mut cooldowns = ROUTE_COOLDOWNS.lock().unwrap();
    cooldowns.retain(|_, cooldown| cooldown.until > now);
    let mut health = cooldowns
        .iter()
        .map(|(route_id, cooldown)| RouteHealth {
            route_id: route_id.clone(),
            state: "cooldown".to_owned(),
            reason: cooldown.reason.clone(),
            retry_after_secs: cooldown.until.saturating_duration_since(now).as_secs(),
        })
        .collect::<Vec<_>>();
    health.sort_by(|left, right| left.route_id.cmp(&right.route_id));
    health
}

#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub route_id: String,
    pub account_id: String,
    pub account_name: String,
    pub provider: ProviderId,
    pub model_id: String,
    pub base_url: String,
    pub credential: Option<String>,
    pub context_window: Option<u64>,
}

impl ResolvedRoute {
    pub fn display_name(&self) -> String {
        format!(
            "{} / {} / {}",
            self.provider.display_name(),
            self.account_name,
            self.model_id
        )
    }
}

#[derive(Debug, Clone)]
pub struct AccountManager {
    file: AccountsFile,
}

impl AccountManager {
    pub fn load() -> Result<Self> {
        Ok(Self {
            file: load_accounts()?,
        })
    }
    pub fn accounts(&self) -> &[AccountEntry] {
        &self.file.accounts
    }
    pub fn enabled_accounts(&self) -> Vec<&AccountEntry> {
        self.file.accounts.iter().filter(|a| a.enabled).collect()
    }
    pub fn get_account(&self, name: &str) -> Option<&AccountEntry> {
        self.file.accounts.iter().find(|a| a.name == name)
    }
    pub fn get_account_by_id(&self, id: &str) -> Option<&AccountEntry> {
        self.file.accounts.iter().find(|account| account.id == id)
    }
    pub fn snapshot(&self) -> AccountSnapshot {
        AccountSnapshot {
            version: self.file.version,
            generation: self.file.generation,
            accounts: self
                .file
                .accounts
                .iter()
                .map(AccountSummary::from)
                .collect(),
            providers: provider_summaries(&self.file.accounts),
            fallback_chain: self.file.fallback_chain.clone(),
            model_preferences: self.file.model_preferences.clone(),
            route_health: route_health(),
        }
    }
    pub fn add(
        &mut self,
        name: String,
        provider: ProviderId,
        api_key: String,
        base_url: Option<String>,
        catalog: Option<String>,
    ) -> Result<AccountEntry> {
        let entry = add_account(name, provider, api_key, base_url, catalog)?;
        self.reload()?;
        Ok(entry)
    }
    pub fn remove(&mut self, name: &str) -> Result<AccountEntry> {
        let result = remove_account(name)?;
        self.reload()?;
        Ok(result)
    }
    pub fn remove_by_id(&mut self, id: &str) -> Result<AccountEntry> {
        let result = remove_account_by_id(id)?;
        self.reload()?;
        Ok(result)
    }
    pub fn edit(
        &mut self,
        id: &str,
        name: Option<String>,
        provider: Option<ProviderId>,
        auth: Option<storage::AccountAuth>,
        base_url: Option<Option<String>>,
        enabled: Option<bool>,
    ) -> Result<AccountEntry> {
        let result = edit_account(id, name, provider, auth, base_url, enabled)?;
        self.reload()?;
        Ok(result)
    }
    pub fn enable(&mut self, name: &str) -> Result<AccountEntry> {
        let result = enable_account(name)?;
        self.reload()?;
        Ok(result)
    }
    pub fn disable(&mut self, name: &str) -> Result<AccountEntry> {
        let result = disable_account(name)?;
        self.reload()?;
        Ok(result)
    }
    pub fn reorder(&mut self, name: &str, position: usize) -> Result<()> {
        reorder_account(name, position)?;
        self.reload()
    }
    pub fn set_fallback_chain(&mut self, chain: Vec<FallbackRoute>) -> Result<()> {
        replace_fallback_chain(chain)?;
        self.reload()
    }
    pub fn update_preferences(
        &mut self,
        route: &str,
        favorite: Option<bool>,
        recent: bool,
    ) -> Result<ModelPreferences> {
        let result = update_model_preferences(route, favorite, recent)?;
        self.reload()?;
        Ok(result)
    }
    pub fn update_models(
        &mut self,
        account_id: &str,
        discovered: Option<Vec<storage::DiscoveredModel>>,
        allowlist: Option<Vec<String>>,
    ) -> Result<AccountEntry> {
        let result = update_account_models(account_id, discovered, allowlist)?;
        self.reload()?;
        Ok(result)
    }
    pub fn enabled_count(&self) -> usize {
        self.enabled_accounts().len()
    }
    pub fn total_count(&self) -> usize {
        self.file.accounts.len()
    }
    pub fn has_accounts(&self) -> bool {
        !self.file.accounts.is_empty()
    }
    pub fn has_enabled_accounts(&self) -> bool {
        self.file.accounts.iter().any(|a| a.enabled)
    }

    pub fn fallback_routes(&self) -> Vec<ResolvedRoute> {
        self.file
            .fallback_chain
            .iter()
            .filter(|route| route.enabled)
            .filter_map(|route| {
                let account = self
                    .file
                    .accounts
                    .iter()
                    .find(|a| a.id == route.account_id && a.enabled)?;
                Some(resolve_route(account, &route.model_id))
            })
            .collect()
    }

    pub fn all_model_routes(&self) -> Vec<ResolvedRoute> {
        self.file
            .accounts
            .iter()
            .filter(|account| account.enabled)
            .flat_map(|account| {
                let models = if account.model_allowlist.is_empty() {
                    account
                        .discovered_models
                        .iter()
                        .map(|m| m.id.as_str())
                        .collect::<Vec<_>>()
                } else {
                    account
                        .model_allowlist
                        .iter()
                        .map(String::as_str)
                        .collect::<Vec<_>>()
                };
                models
                    .into_iter()
                    .map(|model| resolve_route(account, model))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    pub fn find_enabled_route(&self, base_url: &str, model_id: &str) -> Option<ResolvedRoute> {
        let base_url = base_url.trim_end_matches('/');
        self.all_model_routes().into_iter().find(|route| {
            route.model_id == model_id && route.base_url.trim_end_matches('/') == base_url
        })
    }

    pub fn format_list(&self) -> String {
        if self.file.accounts.is_empty() {
            return concat!(
                "No accounts configured.\n\n",
                "Run `/connect add <name> <provider> <key> [model]` or open `/connect` in the TUI.\n",
                "Providers: zen, go, openai, codex, anthropic, ollama, or a custom provider id."
            ).to_owned();
        }
        let mut lines = vec![format!(
            "Connected accounts ({}/{} enabled) · schema v{} · generation {}",
            self.enabled_count(),
            self.total_count(),
            self.file.version,
            self.file.generation
        )];
        for (index, account) in self.file.accounts.iter().enumerate() {
            let state = if account.enabled { "✓" } else { "○" };
            let models = if account.model_allowlist.is_empty() {
                "all models".to_owned()
            } else {
                account.model_allowlist.join(", ")
            };
            lines.push(format!(
                "  {}. {} {} — {} — {}",
                index + 1,
                state,
                account.name,
                account.provider.display_name(),
                models
            ));
        }
        lines.push(String::new());
        lines.push("Fallback chain:".to_owned());
        if self.file.fallback_chain.is_empty() {
            lines.push("  (not configured)".to_owned());
        } else {
            for (index, route) in self.fallback_routes().iter().enumerate() {
                lines.push(format!("  {}. {}", index + 1, route.display_name()));
            }
        }
        lines.push(String::new());
        lines.push("Use `/connect status` for build and storage diagnostics.".to_owned());
        lines.join("\n")
    }

    pub fn format_model_routes(&self) -> String {
        let routes = self.all_model_routes();
        if routes.is_empty() {
            return "No model routes are available. Open `/connect` to add an account and refresh its models."
                .to_owned();
        }
        let preferences = &self.file.model_preferences;
        let mut lines = vec!["Model routes:".to_owned()];
        for heading in ["Recent", "Favorites", "All routes"] {
            lines.push(String::new());
            lines.push(format!("{heading}:"));
            let selected: Vec<&ResolvedRoute> = match heading {
                "Recent" => preferences
                    .recents
                    .iter()
                    .filter_map(|route_id| routes.iter().find(|route| &route.route_id == route_id))
                    .collect(),
                "Favorites" => preferences
                    .favorites
                    .iter()
                    .filter_map(|route_id| routes.iter().find(|route| &route.route_id == route_id))
                    .collect(),
                _ => routes.iter().collect(),
            };
            let mut count = 0;
            for route in selected {
                count += 1;
                lines.push(format!("  {} — {}", route.route_id, route.display_name()));
            }
            if count == 0 {
                lines.push("  (none)".to_owned());
            }
        }
        lines.push(String::new());
        lines.push("Select for this session with `/models <route-id>`.".to_owned());
        lines.join("\n")
    }

    pub fn format_account(account: &AccountEntry) -> String {
        format!(
            "{} ({}) - {}",
            account.name,
            account.provider.as_str(),
            if account.enabled {
                "enabled"
            } else {
                "disabled"
            }
        )
    }
    pub fn format_error(error: &anyhow::Error) -> String {
        format!("Error: {error}")
    }
    pub fn format_success(message: &str) -> String {
        format!("✓ {message}")
    }

    fn reload(&mut self) -> Result<()> {
        self.file = load_accounts()?;
        Ok(())
    }
}

fn resolve_route(account: &AccountEntry, model_id: &str) -> ResolvedRoute {
    ResolvedRoute {
        route_id: storage::route_id(&account.id, model_id),
        account_id: account.id.clone(),
        account_name: account.name.clone(),
        provider: account.provider.clone(),
        model_id: model_id.to_owned(),
        base_url: account
            .base_url
            .clone()
            .or_else(|| account.provider.default_base_url().map(str::to_owned))
            .unwrap_or_default(),
        credential: account.auth.credential().map(str::to_owned),
        context_window: account
            .discovered_models
            .iter()
            .find(|model| model.id == model_id)
            .and_then(|model| model.context_window),
    }
}

pub(crate) fn catalog_entry_for_route<'a>(
    route: &ResolvedRoute,
    models: &'a indexmap::IndexMap<String, crate::agent::config::ModelEntry>,
) -> Option<&'a crate::agent::config::ModelEntry> {
    models
        .get(&route.model_id)
        .or_else(|| models.values().find(|entry| entry.model == route.model_id))
}

impl std::fmt::Display for AccountManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} account(s), {} enabled",
            self.total_count(),
            self.enabled_count()
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_never_contains_credentials() {
        let file = AccountsFile {
            accounts: vec![AccountEntry {
                id: "account-a".into(),
                name: "Zen A".into(),
                provider: ProviderId::parse("zen"),
                auth: storage::AccountAuth::ApiKey {
                    secret: "sk-secret".into(),
                },
                enabled: true,
                base_url: None,
                model_allowlist: vec!["model-a".into()],
                discovered_models: vec![],
                models_refreshed_at: None,
                created_at: None,
                cost_tier: None,
            }],
            ..AccountsFile::default()
        };
        let snapshot = AccountManager { file }.snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("sk-secret"));
        assert!(json.contains("api_key"));
    }

    #[test]
    fn fallback_routes_preserve_requested_account_and_model_order() {
        fn account(id: &str, name: &str, provider: &str, model: &str) -> AccountEntry {
            AccountEntry {
                id: id.into(),
                name: name.into(),
                provider: ProviderId::parse(provider),
                auth: storage::AccountAuth::ApiKey {
                    secret: format!("key-{id}"),
                },
                enabled: true,
                base_url: None,
                model_allowlist: vec![model.into()],
                discovered_models: vec![],
                models_refreshed_at: None,
                created_at: None,
                cost_tier: None,
            }
        }
        let accounts = vec![
            account("zen-a", "Zen A", "zen", "claude"),
            account("go-a", "Go A", "go", "kimi"),
            account("zen-b", "Zen B", "zen", "claude"),
            account("openai", "OpenAI", "openai", "gpt-5"),
            AccountEntry {
                auth: storage::AccountAuth::None,
                ..account("ollama", "Ollama", "ollama", "local")
            },
        ];
        let fallback_chain = accounts
            .iter()
            .map(|account| FallbackRoute {
                account_id: account.id.clone(),
                model_id: account.model_allowlist[0].clone(),
                enabled: true,
            })
            .collect();
        let manager = AccountManager {
            file: AccountsFile {
                accounts,
                fallback_chain,
                ..AccountsFile::default()
            },
        };
        assert_eq!(
            manager
                .fallback_routes()
                .iter()
                .map(|route| route.route_id.as_str())
                .collect::<Vec<_>>(),
            [
                "zen-a::claude",
                "go-a::kimi",
                "zen-b::claude",
                "openai::gpt-5",
                "ollama::local"
            ]
        );
    }
}
