//! `/ultra` -- forward the orchestration status command to the shell.

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Show Ultra orchestration status.
pub struct UltraCommand;

impl SlashCommand for UltraCommand {
    fn name(&self) -> &str {
        "ultra"
    }

    fn description(&self) -> &str {
        "Show Ultra orchestration status"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/ultra [status|on|off]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("status|on|off")
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        CommandResult::PassThrough(super::raw_command_text(self.name(), args))
    }
}
