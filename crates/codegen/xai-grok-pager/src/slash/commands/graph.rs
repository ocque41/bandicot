//! `/graph` -- forward AgentGraph commands to the shell.

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Validate, preview, or manage AgentGraph runs.
pub struct GraphCommand;

impl SlashCommand for GraphCommand {
    fn name(&self) -> &str {
        "graph"
    }

    fn description(&self) -> &str {
        "Validate, preview, or manage AgentGraph runs"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/graph [status|validate <path>|preview <path>|run|pause|drain|resume|cancel]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("status|validate <path>|preview <path>|run|pause|drain|resume|cancel")
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        CommandResult::PassThrough(super::raw_command_text(self.name(), args))
    }
}
