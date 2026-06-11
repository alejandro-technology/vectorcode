//! `vectorcode uninstall` — remove VectorCode from agent configurations.

use anyhow::Result;
use clap::Args;

use super::install::AgentTarget;

/// Arguments for `vectorcode uninstall`.
#[derive(Args, Debug)]
pub struct UninstallArgs {
    /// Uninstall from a specific agent only.
    #[arg(long, value_enum)]
    pub target: Option<AgentTarget>,
}

/// Execute the `uninstall` command.
///
/// Stub implementation — prints what would be removed.
pub fn execute(args: &UninstallArgs) -> Result<()> {
    let targets: Vec<&AgentTarget> = match &args.target {
        Some(t) => vec![t],
        None => vec![
            &AgentTarget::Opencode,
            &AgentTarget::ClaudeCode,
            &AgentTarget::Cursor,
            &AgentTarget::GeminiCli,
            &AgentTarget::Antigravity,
        ],
    };

    eprintln!("Removing VectorCode from agents:");
    for target in &targets {
        eprintln!("  {}", target.display_name());
    }

    // TODO: Real implementation would:
    // 1. For each agent, find its config file
    // 2. Parse the JSON config
    // 3. Remove the VectorCode MCP server entry from mcpServers
    // 4. Write the config back

    eprintln!();
    eprintln!("Note: Agent auto-configuration is not yet implemented.");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    #[test]
    fn uninstall_args_parse_no_target() {
        let cli = Cli::parse_from(["vectorcode", "uninstall"]);
        match cli.command {
            crate::cli::Commands::Uninstall(args) => {
                assert!(args.target.is_none());
            }
            _ => panic!("Expected Uninstall command"),
        }
    }

    #[test]
    fn uninstall_args_parse_specific_target() {
        let cli = Cli::parse_from(["vectorcode", "uninstall", "--target", "cursor"]);
        match cli.command {
            crate::cli::Commands::Uninstall(args) => {
                assert!(matches!(args.target, Some(AgentTarget::Cursor)));
            }
            _ => panic!("Expected Uninstall command"),
        }
    }

    #[test]
    fn uninstall_execute_succeeds() {
        let args = UninstallArgs { target: None };
        let result = execute(&args);
        assert!(result.is_ok(), "Uninstall stub should succeed");
    }

    #[test]
    fn uninstall_execute_with_specific_target() {
        let args = UninstallArgs {
            target: Some(AgentTarget::GeminiCli),
        };
        let result = execute(&args);
        assert!(result.is_ok());
    }
}
