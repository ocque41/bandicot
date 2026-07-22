//! `/fast` -- forward the session service-tier command to the shell.

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Set or show the OpenAI priority service tier for this session.
pub struct FastCommand;

impl SlashCommand for FastCommand {
    fn name(&self) -> &str {
        "fast"
    }

    fn description(&self) -> &str {
        "Set or show the OpenAI priority service tier for this session"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/fast [on|off|status]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("on|off|status")
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        CommandResult::PassThrough(super::raw_command_text(self.name(), args))
    }
}
