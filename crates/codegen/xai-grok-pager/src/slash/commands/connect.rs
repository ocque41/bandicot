//! `/connect` — pager-local account modal with shell-compatible text commands.

use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

pub struct ConnectCommand;

pub fn management_items() -> Vec<ArgItem> {
    let mut items = vec![
        ArgItem {
            display: "Add OpenCode Zen account".to_owned(),
            match_text: "add provider opencode zen api key".to_owned(),
            insert_text: "add zen-account zen ".to_owned(),
            description: "Prefill label and provider; enter the API key to finish".to_owned(),
        },
        ArgItem {
            display: "Add OpenCode Go account".to_owned(),
            match_text: "add provider opencode go api key".to_owned(),
            insert_text: "add go-account go ".to_owned(),
            description: "Prefill label and provider; enter the API key to finish".to_owned(),
        },
        ArgItem {
            display: "Add OpenAI Platform account".to_owned(),
            match_text: "add provider openai platform api key".to_owned(),
            insert_text: "add openai-account openai ".to_owned(),
            description: "Prefill label and provider; enter the API key to finish".to_owned(),
        },
        ArgItem {
            display: "Add Anthropic account".to_owned(),
            match_text: "add provider anthropic claude api key".to_owned(),
            insert_text: "add anthropic-account anthropic ".to_owned(),
            description: "Prefill label and provider; enter the API key to finish".to_owned(),
        },
        ArgItem {
            display: "Add CLIProxyAPI Codex account".to_owned(),
            match_text: "add provider openai codex subscription cliproxyapi".to_owned(),
            insert_text: "add codex-account codex keyless".to_owned(),
            description: "Use the existing local CLIProxyAPI endpoint".to_owned(),
        },
        ArgItem {
            display: "Add OpenAI-compatible endpoint".to_owned(),
            match_text: "add provider custom openai compatible endpoint base url".to_owned(),
            insert_text: "add custom-account openai_compatible API_KEY https://example.com/v1"
                .to_owned(),
            description: "Edit the provider ID, key, and base URL before submitting".to_owned(),
        },
        ArgItem {
            display: "Add local Ollama account".to_owned(),
            match_text: "add provider ollama local keyless".to_owned(),
            insert_text: "add ollama-local ollama keyless".to_owned(),
            description: "Create the keyless local provider account".to_owned(),
        },
        ArgItem {
            display: "Show account and build status".to_owned(),
            match_text: "status diagnostics build executable store".to_owned(),
            insert_text: "status".to_owned(),
            description: "Show enabled state and executable identity".to_owned(),
        },
        ArgItem {
            display: "List accounts and fallback order".to_owned(),
            match_text: "list accounts fallback routes order models".to_owned(),
            insert_text: "list".to_owned(),
            description: "Show accounts, models, and fallback order".to_owned(),
        },
    ];

    if let Ok(manager) = xai_grok_shell::accounts::AccountManager::load() {
        let snapshot = manager.snapshot();
        let fallback_ids = snapshot
            .fallback_chain
            .iter()
            .map(|route| route.route_id())
            .collect::<std::collections::HashSet<_>>();
        for account in manager.accounts() {
            let model_count = if account.model_allowlist.is_empty() {
                account.discovered_models.len()
            } else {
                account.model_allowlist.len()
            };
            let state = if account.enabled {
                "enabled"
            } else {
                "disabled"
            };
            items.push(ArgItem {
                display: format!(
                    "{} {}",
                    if account.enabled { "Disable" } else { "Enable" },
                    account.name
                ),
                match_text: format!(
                    "{} {} {} {} {} models",
                    account.name,
                    account.provider.as_str(),
                    account.provider.display_name(),
                    state,
                    model_count
                ),
                insert_text: format!(
                    "{} {}",
                    if account.enabled { "disable" } else { "enable" },
                    account.name
                ),
                description: format!(
                    "{} · {} · {} model route(s)",
                    account.provider.display_name(),
                    state,
                    model_count
                ),
            });
            items.push(ArgItem {
                display: format!("Refresh models · {}", account.name),
                match_text: format!("refresh validate models {}", account.name),
                insert_text: format!("refresh {}", account.name),
                description: "Validate credentials and refresh the authenticated catalog"
                    .to_owned(),
            });
            items.push(ArgItem {
                display: format!("Remove {}", account.name),
                match_text: format!(
                    "remove delete {} {}",
                    account.name,
                    account.provider.display_name()
                ),
                insert_text: format!("remove {}", account.name),
                description: "Remove the account and its saved routes".to_owned(),
            });
        }
        for route in manager.all_model_routes() {
            let in_fallback = fallback_ids.contains(&route.route_id);
            items.push(ArgItem {
                display: format!(
                    "{} fallback · {}",
                    if in_fallback { "Remove from" } else { "Add to" },
                    route.display_name()
                ),
                match_text: format!("fallback {} {}", route.route_id, route.display_name()),
                insert_text: format!(
                    "fallback {} {}",
                    if in_fallback { "remove" } else { "add" },
                    route.route_id
                ),
                description: if in_fallback {
                    "Currently in the global fallback chain"
                } else {
                    "Append this exact account/model route"
                }
                .to_owned(),
            });
            if in_fallback {
                for (label, operation) in [
                    ("Move fallback earlier", "up"),
                    ("Move fallback later", "down"),
                ] {
                    items.push(ArgItem {
                        display: format!("{label} · {}", route.display_name()),
                        match_text: format!("fallback order {operation} {}", route.route_id),
                        insert_text: format!("fallback {operation} {}", route.route_id),
                        description: "Reorder the global live fallback chain".to_owned(),
                    });
                }
            }
        }
        if !snapshot.fallback_chain.is_empty() {
            items.push(ArgItem {
                display: "Clear fallback chain".to_owned(),
                match_text: "clear remove all fallback routes".to_owned(),
                insert_text: "fallback clear".to_owned(),
                description: "Keep accounts but disable cross-provider fallback".to_owned(),
            });
        }
    }

    items
}

impl SlashCommand for ConnectCommand {
    fn name(&self) -> &str {
        "connect"
    }

    fn description(&self) -> &str {
        "Manage provider accounts and fallback routes"
    }

    fn usage(&self) -> &str {
        "/connect [list|add <name> <provider> <key>|remove <name>|enable <name>|disable <name>|order <name> <position>|status]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn suggest_args(&self, _ctx: &AppCtx, _args_query: &str) -> Option<Vec<ArgItem>> {
        Some(management_items())
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        if args.trim().is_empty() {
            CommandResult::Action(Action::OpenConnect)
        } else {
            // Text operations remain shell-owned for ACP and headless clients.
            CommandResult::PassThrough(super::raw_command_text(self.name(), args))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;

    #[test]
    fn bare_connect_is_pager_local_and_subcommands_pass_through() {
        let models = ModelState::default();
        let mut ctx = crate::slash::commands::tests::make_ctx(&models);
        assert!(matches!(
            ConnectCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenConnect)
        ));
        assert!(matches!(
            ConnectCommand.run(&mut ctx, "list"),
            CommandResult::PassThrough(text) if text == "/connect list"
        ));
    }
}
