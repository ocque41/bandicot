//! `/swarm` -- forward Swarm graph commands to the shell.

use crate::slash::command::{CommandExecCtx, CommandResult, SlashCommand};

/// Validate Swarm graphs or run the local exact-100 fake benchmark.
pub struct SwarmCommand;

impl SlashCommand for SwarmCommand {
    fn name(&self) -> &str {
        "swarm"
    }

    fn description(&self) -> &str {
        "Validate Swarm graphs or run the local exact-100 fake benchmark"
    }

    fn session_scoped(&self) -> bool {
        true
    }

    fn usage(&self) -> &str {
        "/swarm [status|benchmark --fake [--limit <n>]|validate <path>]"
    }

    fn takes_args(&self) -> bool {
        true
    }

    fn arg_placeholder(&self) -> Option<&str> {
        Some("status|benchmark --fake [--limit <n>]|validate <path>")
    }

    fn run(&self, _ctx: &mut CommandExecCtx, args: &str) -> CommandResult {
        CommandResult::PassThrough(super::raw_command_text(self.name(), args))
    }
}
