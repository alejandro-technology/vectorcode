//! `vectorcode install` — auto-configure agents (spec §12.6).
//!
//! Detects installed AI coding agents and adds the VectorCode MCP server
//! entry to their configuration files. Idempotent — safe to run multiple times.

use anyhow::Result;
use clap::{Args, ValueEnum};
use tracing::info;

/// Supported agent targets for installation.
#[derive(Debug, Clone, ValueEnum, PartialEq, Eq)]
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

    /// Get the config file path for this agent.
    pub(crate) fn config_path(&self) -> Option<std::path::PathBuf> {
        let home = std::env::var("HOME").ok()?;
        let home = std::path::Path::new(&home);

        match self {
            Self::Opencode => {
                // Check project-local first, then global
                let local = std::path::Path::new("opencode.json");
                if local.exists() {
                    return Some(local.to_path_buf());
                }
                Some(home.join(".config/opencode/opencode.json"))
            }
            Self::ClaudeCode => Some(home.join(".claude/claude_desktop_config.json")),
            Self::Cursor => {
                let local = std::path::Path::new(".cursor/mcp.json");
                if local.exists() {
                    return Some(local.to_path_buf());
                }
                Some(home.join(".cursor/mcp.json"))
            }
            Self::GeminiCli => Some(home.join(".gemini/settings.json")),
            Self::Antigravity => Some(home.join(".gemini/antigravity/settings.json")),
        }
    }

    /// Build the MCP server entry for this agent.
    fn mcp_server_entry(&self) -> serde_json::Value {
        let binary_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "vectorcode".to_string());

        serde_json::json!({
            "command": binary_path,
            "args": ["serve", "--mcp"],
            "env": {}
        })
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
/// Detects installed agents and adds the VectorCode MCP server entry
/// to their configuration files. Idempotent — safe to run multiple times.
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

    let mut installed_count = 0;

    eprintln!("Installing VectorCode MCP server for agents:");

    for target in &targets {
        let config_path = match target.config_path() {
            Some(p) => p,
            None => {
                eprintln!(
                    "  {} — skipped (cannot determine config path)",
                    target.display_name()
                );
                continue;
            }
        };

        match install_for_agent(target, &config_path) {
            Ok(installed) => {
                if installed {
                    eprintln!(
                        "  {} — configured ({})",
                        target.display_name(),
                        config_path.display()
                    );
                    installed_count += 1;
                } else {
                    eprintln!("  {} — already configured", target.display_name());
                }
            }
            Err(e) => {
                eprintln!("  {} — error: {e}", target.display_name());
            }
        }
    }

    eprintln!();
    if installed_count > 0 {
        eprintln!("Done. {installed_count} agent(s) configured. Restart your agent to activate VectorCode.");
    } else {
        eprintln!("No changes made. All agents were already configured or not detected.");
    }

    Ok(())
}

/// Install VectorCode for a specific agent. Returns true if config was modified.
pub(crate) fn install_for_agent(
    target: &AgentTarget,
    config_path: &std::path::Path,
) -> Result<bool> {
    // Read existing config or create empty
    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Ensure mcpServers object exists
    if config.get("mcpServers").is_none() {
        config["mcpServers"] = serde_json::json!({});
    }

    let entry = target.mcp_server_entry();

    // Check if already configured with same entry
    if let Some(existing) = config["mcpServers"].get("vectorcode") {
        if existing == &entry {
            return Ok(false); // Already configured identically
        }
    }

    // Add/update the vectorcode entry
    config["mcpServers"]["vectorcode"] = entry;

    // Ensure parent directory exists
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Write config
    let json = serde_json::to_string_pretty(&config)?;
    std::fs::write(config_path, json)?;

    info!(
        "Installed VectorCode for {} at {}",
        target.display_name(),
        config_path.display()
    );
    Ok(true)
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
        assert!(result.is_ok(), "Install should succeed");
    }

    #[test]
    fn install_execute_with_specific_target() {
        let args = InstallArgs {
            target: Some(AgentTarget::Opencode),
        };
        let result = execute(&args);
        assert!(result.is_ok());
    }

    // ─── install_for_agent tests ───────────────────────────────────────

    #[test]
    fn install_creates_config_file_with_mcp_entry() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("opencode.json");

        let result = install_for_agent(&AgentTarget::Opencode, &config_path).unwrap();
        assert!(result, "Should return true when config is created");

        assert!(config_path.exists(), "Config file should be created");

        let content = std::fs::read_to_string(&config_path).unwrap();
        let config: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert!(
            config["mcpServers"]["vectorcode"].is_object(),
            "Should have vectorcode MCP entry"
        );
        assert_eq!(
            config["mcpServers"]["vectorcode"]["args"][0], "serve",
            "Args should include 'serve'"
        );
        assert_eq!(
            config["mcpServers"]["vectorcode"]["args"][1], "--mcp",
            "Args should include '--mcp'"
        );
    }

    #[test]
    fn install_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");

        // First install
        let result1 = install_for_agent(&AgentTarget::Cursor, &config_path).unwrap();
        assert!(result1, "First install should modify config");

        // Second install — should detect already configured
        let result2 = install_for_agent(&AgentTarget::Cursor, &config_path).unwrap();
        assert!(!result2, "Second install should be idempotent (no changes)");
    }

    #[test]
    fn install_preserves_existing_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");

        // Write existing config with other settings
        let existing = serde_json::json!({
            "someSetting": "value",
            "mcpServers": {
                "otherTool": {
                    "command": "other",
                    "args": []
                }
            }
        });
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        install_for_agent(&AgentTarget::GeminiCli, &config_path).unwrap();

        let content = std::fs::read_to_string(&config_path).unwrap();
        let config: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(
            config["someSetting"], "value",
            "Existing settings preserved"
        );
        assert!(
            config["mcpServers"]["otherTool"].is_object(),
            "Other MCP servers preserved"
        );
        assert!(
            config["mcpServers"]["vectorcode"].is_object(),
            "VectorCode entry added"
        );
    }

    #[test]
    fn mcp_server_entry_has_correct_structure() {
        let entry = AgentTarget::Opencode.mcp_server_entry();
        assert!(entry["command"].is_string(), "Should have command field");
        assert!(entry["args"].is_array(), "Should have args array");
        assert!(entry["env"].is_object(), "Should have env object");
    }
}
