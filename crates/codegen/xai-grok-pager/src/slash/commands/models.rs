//! `/models` — browse exact account/model routes and select session-local routes.

use crate::app::actions::Action;
use crate::slash::command::{AppCtx, ArgItem, CommandExecCtx, CommandResult, SlashCommand};

pub struct ModelsCommand;

pub fn route_items() -> Option<Vec<ArgItem>> {
    let manager = xai_grok_shell::accounts::AccountManager::load().ok()?;
    let snapshot = manager.snapshot();
    let routes = manager.all_model_routes();
    let mut items = Vec::new();
    let mut push_section = |section: &str, route_ids: &[String]| {
        for route_id in route_ids {
            if let Some(route) = routes.iter().find(|route| &route.route_id == route_id) {
                items.push(ArgItem {
                    display: format!("{section} · {}", route.display_name()),
                    match_text: format!("{section} {} {}", route.route_id, route.display_name()),
                    insert_text: route.route_id.clone(),
                    description: route.route_id.clone(),
                });
            }
        }
    };
    push_section("Recent", &snapshot.model_preferences.recents);
    push_section("Favorite", &snapshot.model_preferences.favorites);
    for route in &routes {
        items.push(ArgItem {
            display: format!("All · {}", route.display_name()),
            match_text: format!("all {} {}", route.route_id, route.display_name()),
            insert_text: route.route_id.clone(),
            description: route.route_id.clone(),
        });
    }
    for route in &routes {
        let favorite = snapshot
            .model_preferences
            .favorites
            .contains(&route.route_id);
        items.push(ArgItem {
            display: format!(
                "{} favorite · {}",
                if favorite { "Remove" } else { "Add" },
                route.display_name()
            ),
            match_text: format!("favorite {} {}", route.route_id, route.display_name()),
            insert_text: format!(
                "{} {}",
                if favorite { "unfavorite" } else { "favorite" },
                route.route_id
            ),
            description: if favorite {
                "★ Favorite"
            } else {
                "☆ Not favorite"
            }
            .to_owned(),
        });
    }
    Some(items)
}

impl SlashCommand for ModelsCommand {
    fn name(&self) -> &str {
        "models"
    }

    fn description(&self) -> &str {
        "Browse provider/account/model routes"
    }

    fn usage(&self) -> &str {
        "/models [account-id::model-id]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn suggest_args(&self, _ctx: &AppCtx, _args_query: &str) -> Option<Vec<ArgItem>> {
        route_items()
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        if args.trim().is_empty() {
            CommandResult::Action(Action::OpenModels)
        } else {
            CommandResult::PassThrough(super::raw_command_text(self.name(), args))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acp::model_state::ModelState;

    #[test]
    fn bare_models_is_pager_local_and_route_selection_passes_through() {
        let models = ModelState::default();
        let mut ctx = crate::slash::commands::tests::make_ctx(&models);
        assert!(matches!(
            ModelsCommand.run(&mut ctx, ""),
            CommandResult::Action(Action::OpenModels)
        ));
        assert!(matches!(
            ModelsCommand.run(&mut ctx, "account-a::model-a"),
            CommandResult::PassThrough(text) if text == "/models account-a::model-a"
        ));
    }
}
