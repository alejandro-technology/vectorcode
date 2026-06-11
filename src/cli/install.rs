//! `vectorcode install` — auto-configure agents (spec §12.6).

use anyhow::Result;
use clap::{Args, ValueEnum};

/// Supported agent targets for installation.
#[derive(Debug, Clone, ValueEnum)]
pub enum AgentTarget {
    Opencode,
    ClaudeCode,
    Cursor,
    GeminiCli,
    Antigravity,
}

impl AgentTarget {
    pub fn display_name(&self) -> &'static str {
        match self {
            Self::Opencode => "OpenCode",
            Self::ClaudeCode => "Claude Code",
            Self::Cursor => "Cursor",
            Self::GeminiCli => "Gemini CLI",
            Self::Antigravity => "Antigravity",
        }
    }
}

/// Arguments for `vectorcode install`.
#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Install for a specific agent only.
    #[arg(long, value_enum)]
    pub target: Option<AgentTarget>,
}

/// Execute the `install` command (spec §12.6).
///
/// Stub implementation — detects installed agents and prints what would be done.
/// Real implementation would parse/write JSON configs for each agent.
pub fn execute(args: &InstallArgs) -> Result<()> {
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

    eprintln!("Installing VectorCode for agents:");
    for target in &targets {
        let detected = detect_agent(target);
        let status = if detected { "found" } else { "not found" };
        eprintln!("  {} ({})", target.display_name(), status);
    }

    // TODO: Real implementation would:
    // 1. For each detected agent, find its config file
    // 2. Parse the JSON config
    // 3. Add the VectorCode MCP server entry to mcpServers
    // 4. Write the config back
    //
    // Agent config paths:
    // - OpenCode: opencode.json → mcpServers
    // - Claude Code: ~/.claude/claude_desktop_config.json → mcpServers
    // - Cursor: .cursor/mcp.json
    // - Gemini CLI: ~/.gemini/settings.json → mcpServers
    // - Antigravity: ~/.gemini/antigravity/settings.json → mcpServers

    eprintln!();
    eprintln!("Note: Agent auto-configuration is not yet implemented.");
    eprintln!("Manually add VectorCode to your agent's MCP configuration.");

    Ok(())
}

/// Check if an agent is installed by looking for its config file.
fn detect_agent(target: &AgentTarget) -> bool {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return false,
    };

    match target {
        AgentTarget::Opencode => {
            // Check for opencode.json in common locations
            std::path::Path::new(&home)
                .join(".config/opencode/opencode.json")
                .exists()
                || std::path::Path::new("opencode.json").exists()
        }
        AgentTarget::ClaudeCode => std::path::Path::new(&home)
            .join(".claude/claude_desktop_config.json")
            .exists(),
        AgentTarget::Cursor => std::path::Path::new(".cursor/mcp.json").exists(),
        AgentTarget::GeminiCli => std::path::Path::new(&home)
            .join(".gemini/settings.json")
            .exists(),
        AgentTarget::Antigravity => std::path::Path::new(&home)
            .join(".gemini/antigravity/settings.json")
            .exists(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    #[test]
    fn install_args_parse_no_target() {
        let cli = Cli::parse_from(["vectorcode", "install"]);
        match cli.command {
            crate::cli::Commands::Install(args) => {
                assert!(args.target.is_none());
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn install_args_parse_specific_target() {
        let cli = Cli::parse_from(["vectorcode", "install", "--target", "opencode"]);
        match cli.command {
            crate::cli::Commands::Install(args) => {
                assert!(matches!(args.target, Some(AgentTarget::Opencode)));
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn install_args_parse_all_targets() {
        for (name, expected) in [
            ("opencode", "OpenCode"),
            ("claude-code", "Claude Code"),
            ("cursor", "Cursor"),
            ("gemini-cli", "Gemini CLI"),
            ("antigravity", "Antigravity"),
        ] {
            let cli = Cli::parse_from(["vectorcode", "install", "--target", name]);
            match cli.command {
                crate::cli::Commands::Install(args) => {
                    let target = args.target.unwrap();
                    assert_eq!(target.display_name(), expected, "For target: {name}");
                }
                _ => panic!("Expected Install command for: {name}"),
            }
        }
    }

    #[test]
    fn agent_target_display_names() {
        assert_eq!(AgentTarget::Opencode.display_name(), "OpenCode");
        assert_eq!(AgentTarget::ClaudeCode.display_name(), "Claude Code");
        assert_eq!(AgentTarget::Cursor.display_name(), "Cursor");
        assert_eq!(AgentTarget::GeminiCli.display_name(), "Gemini CLI");
        assert_eq!(AgentTarget::Antigravity.display_name(), "Antigravity");
    }

    #[test]
    fn install_execute_succeeds() {
        let args = InstallArgs { target: None };
        let result = execute(&args);
        assert!(result.is_ok(), "Install stub should succeed");
    }

    #[test]
    fn install_execute_with_specific_target() {
        let args = InstallArgs {
            target: Some(AgentTarget::Opencode),
        };
        let result = execute(&args);
        assert!(result.is_ok());
    }
}
