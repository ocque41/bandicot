//! Secure, versioned persistence for provider accounts, model preferences, and fallback routes.

use anyhow::{Context, Result, bail};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

pub const ACCOUNTS_SCHEMA_VERSION: u32 = 2;
pub const MAX_RECENT_ROUTES: usize = 3;

static REGISTRY_GENERATION: LazyLock<Mutex<Vec<tokio::sync::watch::Sender<u64>>>> =
    LazyLock::new(|| Mutex::new(Vec::new()));

/// Subscribe to account-registry changes. Each successful mutation publishes
/// the durable document generation after the atomic replacement completes.
pub fn subscribe_registry_generation() -> tokio::sync::watch::Receiver<u64> {
    let generation = load_accounts()
        .map(|file| file.generation)
        .unwrap_or_default();
    let (sender, receiver) = tokio::sync::watch::channel(generation);
    REGISTRY_GENERATION.lock().unwrap().push(sender);
    receiver
}

fn broadcast_registry_generation(generation: u64) {
    REGISTRY_GENERATION
        .lock()
        .unwrap()
        .retain(|sender| sender.send(generation).is_ok());
}

/// Extensible provider identifier. Built-in providers use the constants below;
/// custom OpenAI-compatible providers keep their configured string unchanged.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(transparent)]
pub struct ProviderId(pub String);

impl std::fmt::Debug for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ProviderId").field(&self.0).finish()
    }
}

impl ProviderId {
    pub const OPENCODE_ZEN: &'static str = "opencode_zen";
    pub const OPENCODE_GO: &'static str = "opencode_go";
    pub const OPENAI_PLATFORM: &'static str = "openai_platform";
    pub const OPENAI_CODEX_PLAN: &'static str = "openai_codex_plan";
    pub const ANTHROPIC: &'static str = "anthropic_messages";
    pub const OLLAMA: &'static str = "ollama";
    pub const APPLE: &'static str = "apple";

    pub fn parse(value: &str) -> Self {
        let lowercase = value.trim().to_ascii_lowercase();
        let normalized = match lowercase.as_str() {
            "zen" => Self::OPENCODE_ZEN,
            "go" => Self::OPENCODE_GO,
            "openai" => Self::OPENAI_PLATFORM,
            "codex" => Self::OPENAI_CODEX_PLAN,
            "anthropic" | "claude" => Self::ANTHROPIC,
            other => other,
        };
        Self(normalized.to_owned())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn display_name(&self) -> &str {
        match self.0.as_str() {
            Self::OPENCODE_ZEN => "OpenCode Zen",
            Self::OPENCODE_GO => "OpenCode Go",
            Self::OPENAI_PLATFORM => "OpenAI Platform",
            Self::OPENAI_CODEX_PLAN => "OpenAI Codex Plan",
            Self::ANTHROPIC => "Anthropic",
            Self::OLLAMA => "Ollama (Local)",
            Self::APPLE => "Apple Foundation Models",
            _ => self.0.as_str(),
        }
    }

    pub fn default_base_url(&self) -> Option<&'static str> {
        match self.0.as_str() {
            Self::OPENCODE_ZEN => Some("https://opencode.ai/zen/v1"),
            Self::OPENCODE_GO => Some("https://opencode.ai/zen/go/v1"),
            Self::OPENAI_PLATFORM => Some("https://api.openai.com/v1"),
            Self::ANTHROPIC => Some("https://api.anthropic.com/v1"),
            Self::OLLAMA => Some("http://127.0.0.1:11434/v1"),
            Self::OPENAI_CODEX_PLAN => Some("http://127.0.0.1:8317/v1"),
            _ => None,
        }
    }
}

impl std::fmt::Display for ProviderId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.display_name())
    }
}

/// Credential material is tagged so keyless and externally-managed routes do
/// not need fake API keys. Debug intentionally never prints a secret.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AccountAuth {
    ApiKey { secret: String },
    ExternalProxy { client_token: String },
    None,
}

impl std::fmt::Debug for AccountAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApiKey { .. } => f.write_str("ApiKey { secret: [REDACTED] }"),
            Self::ExternalProxy { .. } => f.write_str("ExternalProxy { client_token: [REDACTED] }"),
            Self::None => f.write_str("None"),
        }
    }
}

impl AccountAuth {
    pub fn credential(&self) -> Option<&str> {
        match self {
            Self::ApiKey { secret } => Some(secret),
            Self::ExternalProxy { client_token } => Some(client_token),
            Self::None => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DiscoveredModel {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountEntry {
    pub id: String,
    pub name: String,
    pub provider: ProviderId,
    pub auth: AccountAuth,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Empty means every discovered/configured model is exposed.
    #[serde(default)]
    pub model_allowlist: Vec<String>,
    #[serde(default)]
    pub discovered_models: Vec<DiscoveredModel>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models_refreshed_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_tier: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FallbackRoute {
    pub account_id: String,
    pub model_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl FallbackRoute {
    pub fn route_id(&self) -> String {
        route_id(&self.account_id, &self.model_id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ModelPreferences {
    #[serde(default)]
    pub favorites: Vec<String>,
    #[serde(default)]
    pub recents: Vec<String>,
}

impl ModelPreferences {
    pub fn touch_recent(&mut self, route: String) {
        self.recents.retain(|item| item != &route);
        self.recents.insert(0, route);
        self.recents.truncate(MAX_RECENT_ROUTES);
    }

    pub fn set_favorite(&mut self, route: String, favorite: bool) {
        self.favorites.retain(|item| item != &route);
        if favorite {
            self.favorites.push(route);
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountsFile {
    pub version: u32,
    #[serde(default)]
    pub generation: u64,
    #[serde(default)]
    pub accounts: Vec<AccountEntry>,
    #[serde(default)]
    pub fallback_chain: Vec<FallbackRoute>,
    #[serde(default)]
    pub model_preferences: ModelPreferences,
}

impl Default for AccountsFile {
    fn default() -> Self {
        Self {
            version: ACCOUNTS_SCHEMA_VERSION,
            generation: 0,
            accounts: Vec::new(),
            fallback_chain: Vec::new(),
            model_preferences: ModelPreferences::default(),
        }
    }
}

#[derive(Deserialize)]
struct LegacyAccountsFile {
    #[serde(default)]
    accounts: Vec<LegacyAccountEntry>,
}

#[derive(Deserialize)]
struct LegacyAccountEntry {
    name: String,
    provider: serde_json::Value,
    #[serde(default)]
    api_key: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    catalog: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    cost_tier: Option<String>,
}

fn default_true() -> bool {
    true
}

pub fn route_id(account_id: &str, model_id: &str) -> String {
    format!("{account_id}::{model_id}")
}

pub fn accounts_path() -> PathBuf {
    crate::util::grok_home::grok_home().join("accounts.json")
}

fn legacy_accounts_path() -> Option<PathBuf> {
    let path = dirs::home_dir()?.join(".grok").join("accounts.json");
    (path != accounts_path()).then_some(path)
}

fn lock_path(path: &Path) -> PathBuf {
    path.with_extension("json.lock")
}

fn acquire_lock(path: &Path) -> Result<File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let lock_file = lock_path(path);
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&lock_file)?;
    tighten_permissions(&lock_file);
    lock.lock_exclusive()
        .context("failed to lock accounts store")?;
    Ok(lock)
}

pub fn load_accounts() -> Result<AccountsFile> {
    load_accounts_at(&accounts_path(), legacy_accounts_path().as_deref())
}

fn load_accounts_at(path: &Path, legacy_path: Option<&Path>) -> Result<AccountsFile> {
    let _lock = acquire_lock(path)?;
    if !path.exists() {
        if let Some(legacy) = legacy_path.filter(|candidate| candidate.exists()) {
            let imported = read_accounts_file(legacy)
                .with_context(|| format!("failed to import legacy {}", legacy.display()))?;
            write_accounts_unlocked(path, &imported)?;
            return Ok(imported);
        }
        return Ok(AccountsFile::default());
    }
    match read_accounts_file(path) {
        Ok(file) => Ok(file),
        Err(error) => {
            let backup = backup_corrupt(path)?;
            tracing::warn!(%error, backup = %backup.display(), "backed up corrupt accounts store");
            Ok(AccountsFile::default())
        }
    }
}

fn read_accounts_file(path: &Path) -> Result<AccountsFile> {
    let file = File::open(path).with_context(|| format!("failed to read {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_reader(BufReader::new(file))
        .with_context(|| format!("failed to parse {}", path.display()))?;
    if value.get("version").is_some() {
        let mut current: AccountsFile = serde_json::from_value(value)?;
        if current.version > ACCOUNTS_SCHEMA_VERSION {
            bail!(
                "accounts schema {} is newer than supported schema {}",
                current.version,
                ACCOUNTS_SCHEMA_VERSION
            );
        }
        current.version = ACCOUNTS_SCHEMA_VERSION;
        current
            .model_preferences
            .recents
            .truncate(MAX_RECENT_ROUTES);
        tighten_permissions(path);
        return Ok(current);
    }
    let legacy: LegacyAccountsFile = serde_json::from_value(value)?;
    Ok(migrate_legacy(legacy))
}

fn migrate_legacy(legacy: LegacyAccountsFile) -> AccountsFile {
    let accounts = legacy
        .accounts
        .into_iter()
        .map(|old| {
            let provider_text = old.provider.as_str().map(str::to_owned).unwrap_or_else(|| {
                serde_json::to_string(&old.provider)
                    .unwrap_or_else(|_| "other".to_owned())
                    .trim_matches('"')
                    .to_owned()
            });
            let provider = ProviderId::parse(&provider_text);
            let auth = if provider.as_str() == ProviderId::OLLAMA
                || provider.as_str() == ProviderId::APPLE
            {
                AccountAuth::None
            } else if provider.as_str() == ProviderId::OPENAI_CODEX_PLAN {
                AccountAuth::ExternalProxy {
                    client_token: old.api_key,
                }
            } else {
                AccountAuth::ApiKey {
                    secret: old.api_key,
                }
            };
            AccountEntry {
                id: uuid::Uuid::now_v7().to_string(),
                name: old.name,
                provider,
                auth,
                enabled: old.enabled,
                base_url: old.base_url,
                model_allowlist: old.catalog.into_iter().collect(),
                discovered_models: Vec::new(),
                models_refreshed_at: None,
                created_at: old.created_at,
                cost_tier: old.cost_tier,
            }
        })
        .collect::<Vec<_>>();
    let fallback_chain = accounts
        .iter()
        .filter(|a| a.enabled)
        .filter_map(|a| {
            a.model_allowlist.first().map(|model| FallbackRoute {
                account_id: a.id.clone(),
                model_id: model.clone(),
                enabled: true,
            })
        })
        .collect();
    AccountsFile {
        accounts,
        fallback_chain,
        ..AccountsFile::default()
    }
}

pub fn save_accounts(file: &AccountsFile) -> Result<()> {
    let path = accounts_path();
    let _lock = acquire_lock(&path)?;
    write_accounts_unlocked(&path, file)?;
    broadcast_registry_generation(file.generation);
    Ok(())
}

fn write_accounts_unlocked(path: &Path, file: &AccountsFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_file_name(format!(
        ".accounts.{}.{}.tmp",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| -> Result<()> {
        let output = crate::util::secure_file::open_secure_file(&temp)?;
        let mut writer = BufWriter::new(output);
        serde_json::to_writer_pretty(&mut writer, file)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        writer
            .into_inner()
            .map_err(|e| e.into_error())?
            .sync_all()?;
        std::fs::rename(&temp, path)?;
        tighten_permissions(path);
        if let Some(parent) = path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    result.with_context(|| format!("failed to save {}", path.display()))
}

fn tighten_permissions(path: &Path) {
    if let Err(error) = crate::util::secure_file::ensure_owner_only_permissions(path) {
        tracing::warn!(%error, path = %path.display(), "failed to enforce accounts permissions");
    }
}

fn mutate_accounts<T>(op: impl FnOnce(&mut AccountsFile) -> Result<T>) -> Result<T> {
    let path = accounts_path();
    mutate_accounts_at(&path, op)
}

fn mutate_accounts_at<T>(
    path: &Path,
    op: impl FnOnce(&mut AccountsFile) -> Result<T>,
) -> Result<T> {
    let _lock = acquire_lock(&path)?;
    let mut file = if path.exists() {
        match read_accounts_file(&path) {
            Ok(file) => file,
            Err(error) => {
                backup_corrupt(&path)?;
                tracing::warn!(%error, "backed up corrupt accounts store before recovery");
                AccountsFile::default()
            }
        }
    } else {
        AccountsFile::default()
    };
    let result = op(&mut file)?;
    file.version = ACCOUNTS_SCHEMA_VERSION;
    file.generation = file.generation.saturating_add(1);
    write_accounts_unlocked(&path, &file)?;
    broadcast_registry_generation(file.generation);
    Ok(result)
}

fn backup_corrupt(path: &Path) -> Result<PathBuf> {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let backup = path.with_file_name(format!("accounts.json.corrupt.{millis}"));
    std::fs::rename(path, &backup)?;
    tighten_permissions(&backup);
    Ok(backup)
}

pub fn add_account(
    name: String,
    provider: ProviderId,
    api_key: String,
    base_url: Option<String>,
    catalog: Option<String>,
) -> Result<AccountEntry> {
    mutate_accounts(|file| {
        if file
            .accounts
            .iter()
            .any(|a| a.name.eq_ignore_ascii_case(&name))
        {
            bail!("account '{name}' already exists");
        }
        let auth =
            if provider.as_str() == ProviderId::OLLAMA || provider.as_str() == ProviderId::APPLE {
                AccountAuth::None
            } else if provider.as_str() == ProviderId::OPENAI_CODEX_PLAN {
                AccountAuth::ExternalProxy {
                    client_token: api_key,
                }
            } else {
                AccountAuth::ApiKey { secret: api_key }
            };
        let entry = AccountEntry {
            id: uuid::Uuid::now_v7().to_string(),
            name: name.clone(),
            provider,
            auth,
            enabled: true,
            base_url,
            model_allowlist: catalog.into_iter().collect(),
            discovered_models: Vec::new(),
            models_refreshed_at: None,
            created_at: Some(chrono::Utc::now().to_rfc3339()),
            cost_tier: None,
        };
        file.accounts.push(entry.clone());
        Ok(entry)
    })
}

pub fn remove_account(name: &str) -> Result<AccountEntry> {
    mutate_accounts(|file| {
        let index = file
            .accounts
            .iter()
            .position(|a| a.name == name)
            .with_context(|| format!("account '{name}' not found"))?;
        let removed = file.accounts.remove(index);
        file.fallback_chain.retain(|r| r.account_id != removed.id);
        let prefix = format!("{}::", removed.id);
        file.model_preferences
            .favorites
            .retain(|r| !r.starts_with(&prefix));
        file.model_preferences
            .recents
            .retain(|r| !r.starts_with(&prefix));
        Ok(removed)
    })
}

fn set_enabled(name: &str, enabled: bool) -> Result<AccountEntry> {
    mutate_accounts(|file| {
        let entry = file
            .accounts
            .iter_mut()
            .find(|a| a.name == name)
            .with_context(|| format!("account '{name}' not found"))?;
        entry.enabled = enabled;
        Ok(entry.clone())
    })
}

pub fn enable_account(name: &str) -> Result<AccountEntry> {
    set_enabled(name, true)
}
pub fn disable_account(name: &str) -> Result<AccountEntry> {
    set_enabled(name, false)
}

pub fn reorder_account(name: &str, new_position: usize) -> Result<()> {
    mutate_accounts(|file| {
        let old = file
            .accounts
            .iter()
            .position(|a| a.name == name)
            .with_context(|| format!("account '{name}' not found"))?;
        let entry = file.accounts.remove(old);
        let index = new_position.min(file.accounts.len());
        file.accounts.insert(index, entry);
        Ok(())
    })
}

pub fn replace_fallback_chain(chain: Vec<FallbackRoute>) -> Result<()> {
    mutate_accounts(|file| {
        for route in &chain {
            let account = file
                .accounts
                .iter()
                .find(|a| a.id == route.account_id)
                .with_context(|| format!("fallback account '{}' not found", route.account_id))?;
            if route.model_id.trim().is_empty() {
                bail!("fallback model cannot be empty");
            }
            if !account.model_allowlist.is_empty()
                && !account.model_allowlist.contains(&route.model_id)
            {
                bail!(
                    "model '{}' is not enabled for account '{}'",
                    route.model_id,
                    account.name
                );
            }
        }
        file.fallback_chain = chain;
        Ok(())
    })
}

pub fn update_model_preferences(
    route: &str,
    favorite: Option<bool>,
    touch_recent: bool,
) -> Result<ModelPreferences> {
    mutate_accounts(|file| {
        if let Some(value) = favorite {
            file.model_preferences.set_favorite(route.to_owned(), value);
        }
        if touch_recent {
            file.model_preferences.touch_recent(route.to_owned());
        }
        Ok(file.model_preferences.clone())
    })
}

pub fn edit_account(
    account_id: &str,
    name: Option<String>,
    provider: Option<ProviderId>,
    auth: Option<AccountAuth>,
    base_url: Option<Option<String>>,
    enabled: Option<bool>,
) -> Result<AccountEntry> {
    mutate_accounts(|file| {
        if let Some(ref candidate) = name
            && file.accounts.iter().any(|account| {
                account.id != account_id && account.name.eq_ignore_ascii_case(candidate)
            })
        {
            bail!("account '{candidate}' already exists");
        }
        let account = file
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .with_context(|| format!("account '{account_id}' not found"))?;
        if let Some(name) = name {
            account.name = name;
        }
        if let Some(provider) = provider {
            account.provider = provider;
        }
        if let Some(auth) = auth {
            account.auth = auth;
        }
        if let Some(base_url) = base_url {
            account.base_url = base_url;
        }
        if let Some(enabled) = enabled {
            account.enabled = enabled;
        }
        Ok(account.clone())
    })
}

pub fn remove_account_by_id(account_id: &str) -> Result<AccountEntry> {
    mutate_accounts(|file| {
        let index = file
            .accounts
            .iter()
            .position(|account| account.id == account_id)
            .with_context(|| format!("account '{account_id}' not found"))?;
        let removed = file.accounts.remove(index);
        file.fallback_chain
            .retain(|route| route.account_id != removed.id);
        let prefix = format!("{}::", removed.id);
        file.model_preferences
            .favorites
            .retain(|route| !route.starts_with(&prefix));
        file.model_preferences
            .recents
            .retain(|route| !route.starts_with(&prefix));
        Ok(removed)
    })
}

pub fn update_account_models(
    account_id: &str,
    discovered_models: Option<Vec<DiscoveredModel>>,
    model_allowlist: Option<Vec<String>>,
) -> Result<AccountEntry> {
    mutate_accounts(|file| {
        let account = file
            .accounts
            .iter_mut()
            .find(|account| account.id == account_id)
            .with_context(|| format!("account '{account_id}' not found"))?;
        if let Some(mut models) = discovered_models {
            models.sort_by(|left, right| left.id.cmp(&right.id));
            models.dedup_by(|left, right| left.id == right.id);
            account.discovered_models = models;
            account.models_refreshed_at = Some(chrono::Utc::now().to_rfc3339());
        }
        if let Some(mut allowlist) = model_allowlist {
            allowlist.retain(|model| !model.trim().is_empty());
            allowlist.sort();
            allowlist.dedup();
            account.model_allowlist = allowlist;
        }
        Ok(account.clone())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_ids_are_extensible() {
        assert_eq!(ProviderId::parse("zen").as_str(), ProviderId::OPENCODE_ZEN);
        assert_eq!(ProviderId::parse("my_company").as_str(), "my_company");
    }

    #[test]
    fn secrets_are_redacted_from_debug() {
        let auth = AccountAuth::ApiKey {
            secret: "sk-secret".into(),
        };
        let debug = format!("{auth:?}");
        assert!(!debug.contains("sk-secret"));
        assert!(debug.contains("REDACTED"));
    }

    #[test]
    fn recents_keep_exactly_three_unique_routes() {
        let mut prefs = ModelPreferences::default();
        for route in ["a::1", "b::2", "c::3", "d::4", "b::2"] {
            prefs.touch_recent(route.into());
        }
        assert_eq!(prefs.recents, ["b::2", "d::4", "c::3"]);
    }

    #[test]
    fn legacy_document_migrates_without_losing_secret() {
        let value = serde_json::json!({"accounts": [{
            "name": "zen-a", "provider": "opencode_zen", "api_key": "sk-test",
            "enabled": true, "catalog": "claude-sonnet-4"
        }]});
        let legacy: LegacyAccountsFile = serde_json::from_value(value).unwrap();
        let migrated = migrate_legacy(legacy);
        assert_eq!(migrated.version, ACCOUNTS_SCHEMA_VERSION);
        assert_eq!(migrated.accounts[0].auth.credential(), Some("sk-test"));
        assert_eq!(migrated.fallback_chain.len(), 1);
    }

    #[test]
    fn atomic_round_trip_uses_owner_only_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        let mut file = AccountsFile::default();
        file.generation = 7;
        write_accounts_unlocked(&path, &file).unwrap();
        let loaded = read_accounts_file(&path).unwrap();
        assert_eq!(loaded.generation, 7);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn corrupt_store_is_backed_up_and_recovers_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("accounts.json");
        std::fs::write(&path, b"{not-json").unwrap();
        let recovered = load_accounts_at(&path, None).unwrap();
        assert!(recovered.accounts.is_empty());
        assert!(!path.exists());
        let backups = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("accounts.json.corrupt.")
            })
            .count();
        assert_eq!(backups, 1);
    }

    #[test]
    fn legacy_import_is_non_destructive() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("legacy.json");
        let target = dir.path().join("new").join("accounts.json");
        std::fs::write(
            &legacy,
            br#"{"accounts":[{"name":"zen-a","provider":"zen","api_key":"sk-test","catalog":"model-a"}]}"#,
        )
        .unwrap();
        let imported = load_accounts_at(&target, Some(&legacy)).unwrap();
        assert_eq!(imported.accounts.len(), 1);
        assert!(legacy.exists(), "legacy file must be preserved");
        assert!(
            target.exists(),
            "imported document must be written to GROK_HOME"
        );
    }

    #[test]
    fn concurrent_writers_do_not_lose_updates() {
        let dir = tempfile::tempdir().unwrap();
        let path = std::sync::Arc::new(dir.path().join("accounts.json"));
        let mut threads = Vec::new();
        for index in 0..12 {
            let path = std::sync::Arc::clone(&path);
            threads.push(std::thread::spawn(move || {
                mutate_accounts_at(&path, |file| {
                    file.model_preferences
                        .favorites
                        .push(format!("account-{index}::model"));
                    Ok(())
                })
                .unwrap();
            }));
        }
        for thread in threads {
            thread.join().unwrap();
        }
        let stored = read_accounts_file(&path).unwrap();
        assert_eq!(stored.generation, 12);
        assert_eq!(stored.model_preferences.favorites.len(), 12);
    }
}
