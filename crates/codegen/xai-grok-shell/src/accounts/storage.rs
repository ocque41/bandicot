//! Persistent account storage for the `/connect` command.
//!
//! Accounts are stored in `~/.grok/accounts.json` with strict file permissions.
//! Each account holds an API key and provider information for the fallback chain.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

/// Known provider types for accounts.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AccountProvider {
    OpencodeZen,
    OpencodeGo,
    OpenaiPlatform,
    OpenaiCodexPlan,
    AnthropicMessages,
    Ollama,
    Apple,
    #[serde(other)]
    Other,
}

impl AccountProvider {
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

    pub fn display_name(self) -> &'static str {
        match self {
            Self::OpencodeZen => "OpenCode Zen",
            Self::OpencodeGo => "OpenCode Go",
            Self::OpenaiPlatform => "OpenAI Platform",
            Self::OpenaiCodexPlan => "OpenAI Codex Plan",
            Self::AnthropicMessages => "Anthropic Messages",
            Self::Ollama => "Ollama (Local)",
            Self::Apple => "Apple Foundation Models",
            Self::Other => "Other",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "opencode_zen" | "zen" => Some(Self::OpencodeZen),
            "opencode_go" | "go" => Some(Self::OpencodeGo),
            "openai_platform" | "openai" => Some(Self::OpenaiPlatform),
            "openai_codex_plan" | "codex" => Some(Self::OpenaiCodexPlan),
            "anthropic_messages" | "anthropic" | "claude" => Some(Self::AnthropicMessages),
            "ollama" => Some(Self::Ollama),
            "apple" => Some(Self::Apple),
            _ => Some(Self::Other),
        }
    }
}

impl std::fmt::Display for AccountProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// A single account entry stored in `accounts.json`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccountEntry {
    /// Human-readable name for this account (e.g., "zen-account-a").
    pub name: String,
    /// Provider type for this account.
    pub provider: AccountProvider,
    /// The API key for this account.
    pub api_key: String,
    /// Whether this account is enabled in the fallback chain.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Optional base URL override for this account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,
    /// Optional catalog model ID override for this account.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub catalog: Option<String>,
    /// When this account was created.
    #[serde(default)]
    pub created_at: Option<String>,
    /// Optional cost tier for UX (`subscription`, `metered`, `local`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_tier: Option<String>,
}

fn default_true() -> bool {
    true
}

/// Top-level structure for `accounts.json`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AccountsFile {
    /// Ordered list of accounts.
    #[serde(default)]
    pub accounts: Vec<AccountEntry>,
}

/// Path to `~/.grok/accounts.json`.
pub fn accounts_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".grok")
        .join("accounts.json")
}

/// Load accounts from `~/.grok/accounts.json`.
pub fn load_accounts() -> Result<AccountsFile> {
    let path = accounts_path();
    if !path.exists() {
        return Ok(AccountsFile::default());
    }

    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;

    let file: AccountsFile = serde_json::from_str(&content)
        .with_context(|| format!("failed to parse {}", path.display()))?;

    Ok(file)
}

/// Save accounts to `~/.grok/accounts.json` with strict permissions.
pub fn save_accounts(file: &AccountsFile) -> Result<()> {
    let path = accounts_path();

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create directory {}", parent.display()))?;
    }

    let content = serde_json::to_string_pretty(file).context("failed to serialize accounts")?;

    // Write to temp file first, then rename for atomicity
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &content).with_context(|| format!("failed to write {}", tmp.display()))?;

    // Set strict permissions before rename (owner-only read/write)
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }

    std::fs::rename(&tmp, &path)
        .with_context(|| format!("failed to rename {} to {}", tmp.display(), path.display()))?;

    Ok(())
}

/// Add a new account to the file.
pub fn add_account(
    name: String,
    provider: AccountProvider,
    api_key: String,
    base_url: Option<String>,
    catalog: Option<String>,
) -> Result<AccountEntry> {
    let mut file = load_accounts()?;

    // Check for duplicate name
    if file.accounts.iter().any(|a| a.name == name) {
        anyhow::bail!("account '{}' already exists", name);
    }

    let entry = AccountEntry {
        name: name.clone(),
        provider,
        api_key,
        enabled: true,
        base_url,
        catalog,
        created_at: Some(chrono::Utc::now().to_rfc3339()),
        cost_tier: None,
    };

    file.accounts.push(entry.clone());
    save_accounts(&file)?;

    Ok(entry)
}

/// Remove an account by name.
pub fn remove_account(name: &str) -> Result<AccountEntry> {
    let mut file = load_accounts()?;

    let idx = file
        .accounts
        .iter()
        .position(|a| a.name == name)
        .with_context(|| format!("account '{}' not found", name))?;

    let removed = file.accounts.remove(idx);
    save_accounts(&file)?;

    Ok(removed)
}

/// Enable an account by name.
pub fn enable_account(name: &str) -> Result<AccountEntry> {
    let mut file = load_accounts()?;

    let entry = file
        .accounts
        .iter_mut()
        .find(|a| a.name == name)
        .with_context(|| format!("account '{}' not found", name))?;

    entry.enabled = true;
    let result = entry.clone();
    save_accounts(&file)?;

    Ok(result)
}

/// Disable an account by name.
pub fn disable_account(name: &str) -> Result<AccountEntry> {
    let mut file = load_accounts()?;

    let entry = file
        .accounts
        .iter_mut()
        .find(|a| a.name == name)
        .with_context(|| format!("account '{}' not found", name))?;

    entry.enabled = false;
    let result = entry.clone();
    save_accounts(&file)?;

    Ok(result)
}

/// Reorder accounts by moving an account to a new position.
pub fn reorder_account(name: &str, new_position: usize) -> Result<()> {
    let mut file = load_accounts()?;

    let old_idx = file
        .accounts
        .iter()
        .position(|a| a.name == name)
        .with_context(|| format!("account '{}' not found", name))?;

    let entry = file.accounts.remove(old_idx);
    let new_idx = new_position.min(file.accounts.len());
    file.accounts.insert(new_idx, entry);

    save_accounts(&file)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_accounts_path() -> PathBuf {
        std::env::temp_dir().join("test_accounts.json")
    }

    #[test]
    fn test_account_provider_from_str() {
        assert_eq!(
            AccountProvider::from_str("zen"),
            Some(AccountProvider::OpencodeZen)
        );
        assert_eq!(
            AccountProvider::from_str("go"),
            Some(AccountProvider::OpencodeGo)
        );
        assert_eq!(
            AccountProvider::from_str("openai"),
            Some(AccountProvider::OpenaiPlatform)
        );
        assert_eq!(
            AccountProvider::from_str("anthropic"),
            Some(AccountProvider::AnthropicMessages)
        );
        assert_eq!(
            AccountProvider::from_str("ollama"),
            Some(AccountProvider::Ollama)
        );
        assert_eq!(
            AccountProvider::from_str("unknown"),
            Some(AccountProvider::Other)
        );
    }

    #[test]
    fn test_account_provider_display() {
        assert_eq!(AccountProvider::OpencodeZen.to_string(), "OpenCode Zen");
        assert_eq!(AccountProvider::OpencodeGo.to_string(), "OpenCode Go");
        assert_eq!(
            AccountProvider::OpenaiPlatform.to_string(),
            "OpenAI Platform"
        );
    }

    #[test]
    fn test_account_entry_serialization() {
        let entry = AccountEntry {
            name: "test".to_string(),
            provider: AccountProvider::OpencodeZen,
            api_key: "sk-test".to_string(),
            enabled: true,
            base_url: None,
            catalog: None,
            created_at: Some("2026-07-21T10:00:00Z".to_string()),
            cost_tier: None,
        };

        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"name\":\"test\""));
        assert!(json.contains("\"provider\":\"opencode_zen\""));
        assert!(json.contains("\"api_key\":\"sk-test\""));
        assert!(json.contains("\"enabled\":true"));

        let deserialized: AccountEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, entry);
    }

    #[test]
    fn test_accounts_file_serialization() {
        let file = AccountsFile {
            accounts: vec![
                AccountEntry {
                    name: "a".to_string(),
                    provider: AccountProvider::OpencodeZen,
                    api_key: "sk-a".to_string(),
                    enabled: true,
                    base_url: None,
                    catalog: None,
                    created_at: None,
                    cost_tier: None,
                },
                AccountEntry {
                    name: "b".to_string(),
                    provider: AccountProvider::OpencodeGo,
                    api_key: "sk-b".to_string(),
                    enabled: false,
                    base_url: Some("https://api.example.com".to_string()),
                    catalog: Some("model-b".to_string()),
                    created_at: None,
                    cost_tier: Some("metered".to_string()),
                },
            ],
        };

        let json = serde_json::to_string_pretty(&file).unwrap();
        let deserialized: AccountsFile = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.accounts.len(), 2);
        assert_eq!(deserialized.accounts[0].name, "a");
        assert_eq!(deserialized.accounts[1].name, "b");
        assert!(!deserialized.accounts[1].enabled);
    }

    #[test]
    fn test_default_accounts_file_is_empty() {
        let file = AccountsFile::default();
        assert!(file.accounts.is_empty());
    }
}
