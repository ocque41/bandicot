//! Account management for the `/connect` command.
//!
//! The `AccountManager` provides a high-level interface for managing API accounts
//! across multiple providers. It coordinates between the interactive TUI, the
//! persistent storage layer, and the fallback chain configuration.

pub mod storage;

use anyhow::{Context, Result};
use storage::{
    AccountEntry, AccountProvider, AccountsFile, add_account, disable_account, enable_account,
    load_accounts, remove_account, reorder_account, save_accounts,
};

/// High-level account manager for the `/connect` command.
#[derive(Debug, Clone)]
pub struct AccountManager {
    file: AccountsFile,
}

impl AccountManager {
    /// Create a new manager by loading accounts from disk.
    pub fn load() -> Result<Self> {
        let file = load_accounts()?;
        Ok(Self { file })
    }

    /// Get all accounts.
    pub fn accounts(&self) -> &[AccountEntry] {
        &self.file.accounts
    }

    /// Get only enabled accounts.
    pub fn enabled_accounts(&self) -> Vec<&AccountEntry> {
        self.file.accounts.iter().filter(|a| a.enabled).collect()
    }

    /// Get a single account by name.
    pub fn get_account(&self, name: &str) -> Option<&AccountEntry> {
        self.file.accounts.iter().find(|a| a.name == name)
    }

    /// Add a new account.
    pub fn add(
        &mut self,
        name: String,
        provider: AccountProvider,
        api_key: String,
        base_url: Option<String>,
        catalog: Option<String>,
    ) -> Result<AccountEntry> {
        let entry = add_account(name, provider, api_key, base_url, catalog)?;
        self.file = load_accounts()?;
        Ok(entry)
    }

    /// Remove an account by name.
    pub fn remove(&mut self, name: &str) -> Result<AccountEntry> {
        let entry = remove_account(name)?;
        self.file = load_accounts()?;
        Ok(entry)
    }

    /// Enable an account by name.
    pub fn enable(&mut self, name: &str) -> Result<AccountEntry> {
        let entry = enable_account(name)?;
        self.file = load_accounts()?;
        Ok(entry)
    }

    /// Disable an account by name.
    pub fn disable(&mut self, name: &str) -> Result<AccountEntry> {
        let entry = disable_account(name)?;
        self.file = load_accounts()?;
        Ok(entry)
    }

    /// Reorder an account to a new position.
    pub fn reorder(&mut self, name: &str, new_position: usize) -> Result<()> {
        reorder_account(name, new_position)?;
        self.file = load_accounts()?;
        Ok(())
    }

    /// Get the count of enabled accounts.
    pub fn enabled_count(&self) -> usize {
        self.file.accounts.iter().filter(|a| a.enabled).count()
    }

    /// Get the count of all accounts.
    pub fn total_count(&self) -> usize {
        self.file.accounts.len()
    }

    /// Check if any accounts exist.
    pub fn has_accounts(&self) -> bool {
        !self.file.accounts.is_empty()
    }

    /// Check if any accounts are enabled.
    pub fn has_enabled_accounts(&self) -> bool {
        self.file.accounts.iter().any(|a| a.enabled)
    }

    /// Format account list for text output (non-interactive).
    pub fn format_list(&self) -> String {
        if self.file.accounts.is_empty() {
            return "No accounts configured.\n\nUse `/connect add <name> <provider> <key>` to add an account.".to_string();
        }

        let mut lines = Vec::new();
        lines.push(format!("Accounts ({}/{} enabled):", self.enabled_count(), self.total_count()));
        lines.push(String::new());

        for (i, account) in self.file.accounts.iter().enumerate() {
            let status = if account.enabled { "✓" } else { "○" };
            let provider = account.provider.as_str();
            let name = &account.name;
            let catalog = account
                .catalog
                .as_deref()
                .unwrap_or(provider);
            lines.push(format!("  {} {}  {}  {}", status, name, catalog, ""));
        }

        lines.push(String::new());
        lines.push("Commands:".to_string());
        lines.push("  /connect add <name> <provider> <key>   Add account".to_string());
        lines.push("  /connect remove <name>                 Remove account".to_string());
        lines.push("  /connect enable <name>                 Enable account".to_string());
        lines.push("  /connect disable <name>                Disable account".to_string());

        lines.join("\n")
    }

    /// Format a single account entry for display.
    pub fn format_account(account: &AccountEntry) -> String {
        let status = if account.enabled { "enabled" } else { "disabled" };
        format!("{} ({}) - {}", account.name, account.provider.as_str(), status)
    }

    /// Format an error message for display.
    pub fn format_error(err: &anyhow::Error) -> String {
        format!("Error: {}", err)
    }

    /// Format a success message for display.
    pub fn format_success(message: &str) -> String {
        format!("✓ {}", message)
    }
}

impl std::fmt::Display for AccountManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.file.accounts.is_empty() {
            write!(f, "No accounts configured")
        } else {
            write!(
                f,
                "{} account(s), {} enabled",
                self.total_count(),
                self.enabled_count()
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_account_manager_new() {
        let manager = AccountManager {
            file: AccountsFile::default(),
        };
        assert!(!manager.has_accounts());
        assert!(!manager.has_enabled_accounts());
        assert_eq!(manager.enabled_count(), 0);
        assert_eq!(manager.total_count(), 0);
    }

    #[test]
    fn test_format_list_empty() {
        let manager = AccountManager {
            file: AccountsFile::default(),
        };
        let list = manager.format_list();
        assert!(list.contains("No accounts configured"));
    }

    #[test]
    fn test_format_account() {
        let account = AccountEntry {
            name: "test".to_string(),
            provider: AccountProvider::OpencodeZen,
            api_key: "sk-test".to_string(),
            enabled: true,
            base_url: None,
            catalog: None,
            created_at: None,
            cost_tier: None,
        };
        let formatted = AccountManager::format_account(&account);
        assert!(formatted.contains("test"));
        assert!(formatted.contains("enabled"));
    }

    #[test]
    fn test_format_account_disabled() {
        let account = AccountEntry {
            name: "test".to_string(),
            provider: AccountProvider::OpencodeGo,
            api_key: "sk-test".to_string(),
            enabled: false,
            base_url: None,
            catalog: None,
            created_at: None,
            cost_tier: None,
        };
        let formatted = AccountManager::format_account(&account);
        assert!(formatted.contains("disabled"));
    }
}
