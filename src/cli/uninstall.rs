//! `vectorcode uninstall` — remove VectorCode from agent configurations.
//!
//! Removes the VectorCode MCP server entry from agent config files.
//! Idempotent — safe to run multiple times.

use anyhow::Result;
use clap::Args;
use tracing::info;

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
/// Removes the VectorCode MCP server entry from each agent's config file.
/// Idempotent — safe to run even if VectorCode was never installed.
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

    let mut removed_count = 0;

    eprintln!("Removing VectorCode from agents:");

    for target in &targets {
        let config_path = match target.config_path() {
            Some(p) => p,
            None => {
                eprintln!("  {} — skipped (no config path)", target.display_name());
                continue;
            }
        };

        if !config_path.exists() {
            eprintln!(
                "  {} — not installed (config not found)",
                target.display_name()
            );
            continue;
        }

        match uninstall_for_agent(target, &config_path) {
            Ok(true) => {
                eprintln!(
                    "  {} — removed ({})",
                    target.display_name(),
                    config_path.display()
                );
                removed_count += 1;
            }
            Ok(false) => {
                eprintln!("  {} — not installed", target.display_name());
            }
            Err(e) => {
                eprintln!("  {} — error: {e}", target.display_name());
            }
        }
    }

    eprintln!();
    if removed_count > 0 {
        eprintln!("Done. Removed VectorCode from {removed_count} agent(s).");
    } else {
        eprintln!("No changes made. VectorCode was not found in any agent config.");
    }

    Ok(())
}

/// Remove VectorCode from a specific agent's config. Returns true if config was modified.
fn uninstall_for_agent(target: &AgentTarget, config_path: &std::path::Path) -> Result<bool> {
    let content = std::fs::read_to_string(config_path)?;
    let mut config: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return Ok(false), // Not valid JSON — nothing to remove
    };

    let mcp_key = target.mcp_config_key();

    // Check if vectorcode entry exists
    let has_entry = config
        .get(mcp_key)
        .and_then(|ms| ms.get("vectorcode"))
        .is_some();

    if !has_entry {
        return Ok(false);
    }

    // Remove the vectorcode entry
    if let Some(mcp_servers) = config.get_mut(mcp_key) {
        if let Some(obj) = mcp_servers.as_object_mut() {
            obj.remove("vectorcode");
        }
    }

    // Write back
    let json = serde_json::to_string_pretty(&config)?;
    std::fs::write(config_path, json)?;

    info!("Removed VectorCode from {}", config_path.display());
    Ok(true)
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
        assert!(result.is_ok(), "Uninstall should succeed");
    }

    #[test]
    fn uninstall_execute_with_specific_target() {
        let args = UninstallArgs {
            target: Some(AgentTarget::GeminiCli),
        };
        let result = execute(&args);
        assert!(result.is_ok());
    }

    // ─── uninstall_for_agent tests ─────────────────────────────────────

    #[test]
    fn uninstall_removes_vectorcode_entry() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");

        // Write config with vectorcode entry (Cursor uses mcpServers)
        let config = serde_json::json!({
            "mcpServers": {
                "vectorcode": {
                    "command": "vectorcode",
                    "args": ["serve", "--mcp"]
                },
                "otherTool": {
                    "command": "other"
                }
            }
        });
        std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        let result = uninstall_for_agent(&AgentTarget::Cursor, &config_path).unwrap();
        assert!(result, "Should return true when entry is removed");

        let content = std::fs::read_to_string(&config_path).unwrap();
        let updated: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert!(
            updated["mcpServers"]["vectorcode"].is_null(),
            "vectorcode entry should be removed"
        );
        assert!(
            updated["mcpServers"]["otherTool"].is_object(),
            "Other entries should be preserved"
        );
    }

    #[test]
    fn uninstall_removes_vectorcode_entry_opencode() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("opencode.json");

        // Write config with vectorcode entry in OpenCode format (uses "mcp" key)
        let config = serde_json::json!({
            "mcp": {
                "vectorcode": {
                    "command": ["vectorcode", "serve", "--mcp"],
                    "type": "local",
                    "enabled": true
                },
                "otherTool": {
                    "command": ["/usr/bin/other"],
                    "type": "local"
                }
            }
        });
        std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        let result = uninstall_for_agent(&AgentTarget::Opencode, &config_path).unwrap();
        assert!(result, "Should return true when entry is removed");

        let content = std::fs::read_to_string(&config_path).unwrap();
        let updated: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert!(
            updated["mcp"]["vectorcode"].is_null(),
            "vectorcode entry should be removed from 'mcp' key"
        );
        assert!(
            updated["mcp"]["otherTool"].is_object(),
            "Other entries under 'mcp' should be preserved"
        );
    }

    #[test]
    fn uninstall_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");

        // Write config without vectorcode
        let config = serde_json::json!({
            "mcpServers": {
                "otherTool": { "command": "other" }
            }
        });
        std::fs::write(&config_path, serde_json::to_string_pretty(&config).unwrap()).unwrap();

        let result = uninstall_for_agent(&AgentTarget::Cursor, &config_path).unwrap();
        assert!(!result, "Should return false when nothing to remove");
    }

    #[test]
    fn uninstall_handles_missing_config() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("nonexistent.json");

        let result = uninstall_for_agent(&AgentTarget::Cursor, &config_path);
        assert!(result.is_err() || !result.unwrap());
    }

    #[test]
    fn uninstall_handles_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "not valid json {{{").unwrap();

        let result = uninstall_for_agent(&AgentTarget::GeminiCli, &config_path).unwrap();
        assert!(!result, "Should return false for invalid JSON");
    }

    #[test]
    fn uninstall_install_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");

        // Install
        let installed = crate::cli::install::install_for_agent(
            &AgentTarget::Opencode,
            &config_path,
            dir.path(),
        )
        .unwrap();
        assert!(installed);

        // Verify entry exists — OpenCode uses "mcp" key
        let content = std::fs::read_to_string(&config_path).unwrap();
        let config: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(config["mcp"]["vectorcode"].is_object());

        // Uninstall
        let removed = uninstall_for_agent(&AgentTarget::Opencode, &config_path).unwrap();
        assert!(removed);

        // Verify entry is gone
        let content = std::fs::read_to_string(&config_path).unwrap();
        let config: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(config["mcp"]["vectorcode"].is_null());
    }

    #[test]
    fn uninstall_install_roundtrip_standard_agent() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("cursor_config.json");

        // Install for Cursor (uses mcpServers)
        let installed =
            crate::cli::install::install_for_agent(&AgentTarget::Cursor, &config_path, dir.path())
                .unwrap();
        assert!(installed);

        // Verify entry exists
        let content = std::fs::read_to_string(&config_path).unwrap();
        let config: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(config["mcpServers"]["vectorcode"].is_object());

        // Uninstall
        let removed = uninstall_for_agent(&AgentTarget::Cursor, &config_path).unwrap();
        assert!(removed);

        // Verify entry is gone
        let content = std::fs::read_to_string(&config_path).unwrap();
        let config: serde_json::Value = serde_json::from_str(&content).unwrap();
        assert!(config["mcpServers"]["vectorcode"].is_null());
    }
}
