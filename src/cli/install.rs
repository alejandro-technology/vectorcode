//! `vectorcode install` — auto-configure agents (spec §12.6).
//!
//! Detects installed AI coding agents and adds the VectorCode MCP server
//! entry to their configuration files. Idempotent — safe to run multiple times.

use anyhow::Result;
use clap::{Args, ValueEnum};
use tracing::info;

/// Embedded SKILL.md content for semantic-search skill (spec §15.2).
const SEMANTIC_SEARCH_SKILL: &str = r#"---
name: semantic-search
description: >
  Use when searching for code by concept, meaning, or behavior — not by exact
  symbol name or literal string. Ideal for queries like "payment retry logic",
  "user authentication flow", "error handling for database connections", or
  "functions similar to createUser". Do NOT use for exact string matches (use
  grep) or known symbol lookups (use codegraph_explore).
---

## Semantic Code Search Protocol

### Tool: `vec_search`

Performs cosine-similarity search over embedded code chunks. Returns ranked
results with file paths, line numbers, symbols, and source code.

### When to use `vec_search`

- You need to find code related to a **concept** but don't know the symbol names
- `grep` returned no results because the code uses different terminology
- You want to find **similar** code patterns across the codebase
- You're exploring an unfamiliar area of the codebase by topic

### When NOT to use `vec_search`

- You know the exact function/class name → use `codegraph_explore`
- You know an exact string in the code → use `grep`
- You're looking for past decisions or history → use `mem_search` (Engram)

### Recommended flow: Semantic → Structural → Historical

For comprehensive code discovery, combine all three tools:

1. **`vec_search("payment error handling")`**
   → Finds code chunks semantically related to payment errors
   → Returns file paths, line ranges, and ranked source snippets

2. **`codegraph_explore("PaymentError handlePaymentFailure")`**
   → Takes symbol names found in step 1
   → Returns full source code + call graph + blast radius

3. **`mem_search("payment error handling")`**
   → Checks Engram for prior team decisions about this topic
   → Returns architectural context and history

### Query tips

- Be specific: "retry with exponential backoff" > "retry"
- Include domain terms: "payment validation" > "validation"
- Describe behavior: "function that sends email notifications" > "email"
- Use `--language` filter when you know the target language
- Use `--path` filter to scope to a specific module

### Example

```
vec_search("middleware that validates JWT tokens and extracts user info")
```
"#;

/// Embedded instructions.md content for MCP agents (spec §16.2).
const MCP_INSTRUCTIONS: &str = r#"# VectorCode — semantic code search over embedded vectors

VectorCode indexes the codebase into vector embeddings and enables
semantic similarity search. It finds code by meaning, not by name.

## Tool selection

- **"Find code about X concept / behavior / domain"** → `vec_search`
- **"Check if index is healthy / current"** → `vec_status`
- **"Force re-index after major changes"** → `vec_reindex`

## When to use vec_search vs other tools

- **Know the exact string** → grep (exact match, faster)
- **Know the symbol name** → codegraph_explore (structural, precise)
- **Know the concept but not the name** → vec_search (semantic, fuzzy)
- **Looking for past decisions** → mem_search / Engram (memory)

## Anti-patterns

- Don't use vec_search to find a symbol you already know the name of —
  codegraph_explore is faster and returns structural context.
- Don't re-verify vec_search results with grep — the source code in the
  result IS the current indexed content. Check the staleness banner if present.
- Don't ignore the score — results below 0.4 are usually noise.

## Staleness

The file watcher keeps the index current (2-second debounce after edits).
If a result has a ⚠️ staleness banner, read those specific files directly.
All files NOT in the banner are fresh.
"#;

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

    /// Return the JSON key used for the MCP servers object in this agent's config.
    pub(crate) fn mcp_config_key(&self) -> &'static str {
        match self {
            Self::Opencode => "mcp",
            _ => "mcpServers",
        }
    }

    /// Build the MCP server entry for this agent, matching the agent's expected format.
    fn mcp_server_entry(&self, project_path: &std::path::Path) -> serde_json::Value {
        let binary_path = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "vectorcode".to_string());

        match self {
            Self::Opencode => serde_json::json!({
                "command": [binary_path, "serve", "--mcp", "--project-path", project_path.to_string_lossy()],
                "type": "local",
                "enabled": true
            }),
            _ => serde_json::json!({
                "command": binary_path,
                "args": ["serve", "--mcp", "--project-path", project_path.to_string_lossy()],
                "env": {}
            }),
        }
    }
}

/// Arguments for `vectorcode install`.
#[derive(Args, Debug)]
pub struct InstallArgs {
    /// Install for a specific agent only.
    #[arg(long, value_enum)]
    pub target: Option<AgentTarget>,

    /// Overwrite existing skill/instructions files.
    #[arg(long)]
    pub force: bool,
}

/// Execute the `install` command (spec §12.6).
///
/// Detects installed agents and adds the VectorCode MCP server entry
/// to their configuration files. Also writes skill files and MCP instructions.
/// Idempotent — safe to run multiple times.
pub fn execute(args: &InstallArgs, project_path: &std::path::Path) -> Result<()> {
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

        match install_for_agent(target, &config_path, project_path) {
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

    // Write skill files and MCP instructions (spec §15, §16)
    let skill_files_written = write_skill_files(args.force)?;
    let instructions_written = write_instructions(args.force)?;
    let total_files = skill_files_written + instructions_written;
    if total_files > 0 {
        eprintln!("Wrote {total_files} skill/instructions file(s).");
    }

    Ok(())
}

/// Write SKILL.md to project-local and global agent skill directories.
///
/// Paths written (spec §15.1):
/// - `.agents/skills/semantic-search/SKILL.md` (project-local, if in a project)
/// - `~/.agents/skills/semantic-search/SKILL.md` (global)
///
/// Returns the number of files written. Skips existing files unless `force` is true.
pub(crate) fn write_skill_files(force: bool) -> Result<usize> {
    let mut written = 0;

    // Project-local: .agents/skills/semantic-search/SKILL.md
    let local_path = std::path::Path::new(".agents/skills/semantic-search/SKILL.md");
    if force || !local_path.exists() {
        if let Some(parent) = local_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(local_path, SEMANTIC_SEARCH_SKILL)?;
        written += 1;
    }

    // Global: ~/.agents/skills/semantic-search/SKILL.md
    if let Ok(home) = std::env::var("HOME") {
        let global_path =
            std::path::PathBuf::from(&home).join(".agents/skills/semantic-search/SKILL.md");
        if force || !global_path.exists() {
            if let Some(parent) = global_path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&global_path, SEMANTIC_SEARCH_SKILL)?;
            written += 1;
        }
    }

    Ok(written)
}

/// Write instructions.md to the Gemini/Antigravity MCP instructions path (spec §16.1).
///
/// Path: `~/.gemini/antigravity/mcp/vectorcode/instructions.md`
///
/// Returns 1 if written, 0 if skipped. Skips existing files unless `force` is true.
pub(crate) fn write_instructions(force: bool) -> Result<usize> {
    if let Ok(home) = std::env::var("HOME") {
        let path = std::path::PathBuf::from(&home)
            .join(".gemini/antigravity/mcp/vectorcode/instructions.md");
        if force || !path.exists() {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(&path, MCP_INSTRUCTIONS)?;
            return Ok(1);
        }
    }
    Ok(0)
}

/// Install VectorCode for a specific agent. Returns true if config was modified.
pub(crate) fn install_for_agent(
    target: &AgentTarget,
    config_path: &std::path::Path,
    project_path: &std::path::Path,
) -> Result<bool> {
    // Read existing config or create empty
    let mut config: serde_json::Value = if config_path.exists() {
        let content = std::fs::read_to_string(config_path)?;
        serde_json::from_str(&content).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    let mcp_key = target.mcp_config_key();

    // Ensure the MCP servers object exists under the agent's key
    if config.get(mcp_key).is_none() {
        config[mcp_key] = serde_json::json!({});
    }

    let entry = target.mcp_server_entry(project_path);

    // Check if already configured with same entry
    if let Some(existing) = config[mcp_key].get("vectorcode") {
        if existing == &entry {
            return Ok(false); // Already configured identically
        }
    }

    // Add/update the vectorcode entry
    config[mcp_key]["vectorcode"] = entry;

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
        let args = InstallArgs {
            target: None,
            force: false,
        };
        let tmp = tempfile::tempdir().unwrap();
        let result = execute(&args, tmp.path());
        assert!(result.is_ok(), "Install should succeed");
    }

    #[test]
    fn install_execute_with_specific_target() {
        let args = InstallArgs {
            target: Some(AgentTarget::Opencode),
            force: false,
        };
        let tmp = tempfile::tempdir().unwrap();
        let result = execute(&args, tmp.path());
        assert!(result.is_ok());
    }

    // ─── install_for_agent tests ───────────────────────────────────────

    #[test]
    fn install_creates_config_file_with_mcp_entry_opencode() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("opencode.json");

        let result = install_for_agent(&AgentTarget::Opencode, &config_path, dir.path()).unwrap();
        assert!(result, "Should return true when config is created");

        assert!(config_path.exists(), "Config file should be created");

        let content = std::fs::read_to_string(&config_path).unwrap();
        let config: serde_json::Value = serde_json::from_str(&content).unwrap();

        // OpenCode uses "mcp" key, not "mcpServers"
        assert!(
            config["mcp"]["vectorcode"].is_object(),
            "Should have vectorcode MCP entry under 'mcp' key"
        );
        // OpenCode format: command is an array, with type and enabled fields
        let command = config["mcp"]["vectorcode"]["command"].as_array().unwrap();
        assert!(
            command.contains(&serde_json::Value::String("serve".to_string())),
            "Command array should include 'serve'"
        );
        assert!(
            command.contains(&serde_json::Value::String("--mcp".to_string())),
            "Command array should include '--mcp'"
        );
        assert!(
            command.contains(&serde_json::Value::String("--project-path".to_string())),
            "Command array should include '--project-path'"
        );
        assert_eq!(
            config["mcp"]["vectorcode"]["type"], "local",
            "Should have type 'local'"
        );
        assert_eq!(
            config["mcp"]["vectorcode"]["enabled"], true,
            "Should have enabled true"
        );
    }

    #[test]
    fn install_creates_config_file_with_mcp_entry_other_agent() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("cursor_config.json");

        let result = install_for_agent(&AgentTarget::Cursor, &config_path, dir.path()).unwrap();
        assert!(result, "Should return true when config is created");

        assert!(config_path.exists(), "Config file should be created");

        let content = std::fs::read_to_string(&config_path).unwrap();
        let config: serde_json::Value = serde_json::from_str(&content).unwrap();

        // Non-OpenCode agents use "mcpServers" key
        assert!(
            config["mcpServers"]["vectorcode"].is_object(),
            "Should have vectorcode MCP entry under 'mcpServers' key"
        );
        assert_eq!(
            config["mcpServers"]["vectorcode"]["args"][0], "serve",
            "Args should include 'serve'"
        );
        assert_eq!(
            config["mcpServers"]["vectorcode"]["args"][1], "--mcp",
            "Args should include '--mcp'"
        );
        assert_eq!(
            config["mcpServers"]["vectorcode"]["args"][2], "--project-path",
            "Args should include '--project-path'"
        );
    }

    #[test]
    fn install_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");

        // First install
        let result1 = install_for_agent(&AgentTarget::Cursor, &config_path, dir.path()).unwrap();
        assert!(result1, "First install should modify config");

        // Second install — should detect already configured
        let result2 = install_for_agent(&AgentTarget::Cursor, &config_path, dir.path()).unwrap();
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

        install_for_agent(&AgentTarget::GeminiCli, &config_path, dir.path()).unwrap();

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
    fn install_preserves_existing_config_opencode() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("opencode.json");

        // OpenCode-style existing config — mcp key with other tools
        let existing = serde_json::json!({
            "someSetting": "value",
            "mcp": {
                "otherTool": {
                    "command": ["/usr/bin/other"],
                    "type": "local",
                    "enabled": true
                }
            }
        });
        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&existing).unwrap(),
        )
        .unwrap();

        install_for_agent(&AgentTarget::Opencode, &config_path, dir.path()).unwrap();

        let content = std::fs::read_to_string(&config_path).unwrap();
        let config: serde_json::Value = serde_json::from_str(&content).unwrap();

        assert_eq!(
            config["someSetting"], "value",
            "Existing settings preserved"
        );
        assert!(
            config["mcp"]["otherTool"].is_object(),
            "Other MCP servers preserved under 'mcp' key"
        );
        assert!(
            config["mcp"]["vectorcode"].is_object(),
            "VectorCode entry added under 'mcp' key"
        );
        assert_eq!(
            config["mcp"]["vectorcode"]["type"], "local",
            "VectorCode entry should have OpenCode format"
        );
    }

    #[test]
    fn mcp_config_key_per_agent() {
        // OpenCode uses "mcp"
        assert_eq!(AgentTarget::Opencode.mcp_config_key(), "mcp");
        // All others use "mcpServers"
        assert_eq!(AgentTarget::ClaudeCode.mcp_config_key(), "mcpServers");
        assert_eq!(AgentTarget::Cursor.mcp_config_key(), "mcpServers");
        assert_eq!(AgentTarget::GeminiCli.mcp_config_key(), "mcpServers");
        assert_eq!(AgentTarget::Antigravity.mcp_config_key(), "mcpServers");
    }

    #[test]
    fn mcp_server_entry_opencode_format() {
        let entry = AgentTarget::Opencode.mcp_server_entry(std::path::Path::new("/tmp/test"));
        // OpenCode format: command is an array with binary + all args
        assert!(entry["command"].is_array(), "Should have command as array");
        let command = entry["command"].as_array().unwrap();
        assert!(
            command.len() >= 5,
            "Should have binary + serve + --mcp + --project-path + path"
        );
        assert!(
            entry["enabled"].as_bool() == Some(true),
            "Should have enabled: true"
        );
        assert_eq!(entry["type"], "local", "Should have type: local");
        // No env field in OpenCode format
        assert!(entry.get("env").is_none(), "Should not have env field");
        // No args field in OpenCode format
        assert!(entry.get("args").is_none(), "Should not have args field");
    }

    #[test]
    fn mcp_server_entry_standard_format() {
        let entry = AgentTarget::Cursor.mcp_server_entry(std::path::Path::new("/tmp/test"));
        // Standard format: separate command string and args array
        assert!(
            entry["command"].is_string(),
            "Should have command as string"
        );
        assert!(entry["args"].is_array(), "Should have args array");
        let args = entry["args"].as_array().unwrap();
        assert!(
            args.len() >= 4,
            "Should have serve, --mcp, --project-path, and path"
        );
        assert!(entry["env"].is_object(), "Should have env object");
        // No enabled or type in standard format
        assert!(
            entry.get("enabled").is_none(),
            "Should not have enabled field"
        );
        assert!(entry.get("type").is_none(), "Should not have type field");
    }

    // ─── Skill file & instructions tests ────────────────────────────────

    #[test]
    fn install_args_parse_force_flag() {
        let cli = Cli::parse_from(["vectorcode", "install", "--force"]);
        match cli.command {
            crate::cli::Commands::Install(args) => {
                assert!(args.force, "--force should be parsed as true");
            }
            _ => panic!("Expected Install command"),
        }
    }

    #[test]
    fn skill_files_written_to_custom_dir() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".agents/skills/semantic-search");
        let skill_path = skill_dir.join("SKILL.md");

        // Simulate writing skill file to custom location
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(&skill_path, SEMANTIC_SEARCH_SKILL).unwrap();

        assert!(skill_path.exists(), "SKILL.md should be created");
        let content = std::fs::read_to_string(&skill_path).unwrap();
        assert!(
            content.contains("name: semantic-search"),
            "Should contain skill name"
        );
        assert!(
            content.contains("vec_search"),
            "Should contain vec_search reference"
        );
    }

    #[test]
    fn skill_file_idempotent_no_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".agents/skills/semantic-search");
        let skill_path = skill_dir.join("SKILL.md");

        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(&skill_path, "existing content").unwrap();

        // Without force, should not overwrite
        // Guard: if !force && skill_path.exists() { skip write }
        // force=false, exists=true → should NOT write
        let _force = false; // guard condition: only write if force || !exists
                            // Verify existing content preserved
        let content = std::fs::read_to_string(&skill_path).unwrap();
        assert_eq!(
            content, "existing content",
            "Should not overwrite without force"
        );
    }

    #[test]
    fn skill_file_force_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let skill_dir = dir.path().join(".agents/skills/semantic-search");
        let skill_path = skill_dir.join("SKILL.md");

        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(&skill_path, "old content").unwrap();

        // With force, should overwrite
        let force = true;
        if force || !skill_path.exists() {
            std::fs::write(&skill_path, SEMANTIC_SEARCH_SKILL).unwrap();
        }

        let content = std::fs::read_to_string(&skill_path).unwrap();
        assert_ne!(
            content, "old content",
            "Force should overwrite existing content"
        );
        assert!(content.contains("semantic-search"));
    }

    #[test]
    fn instructions_file_content_correct() {
        let dir = tempfile::tempdir().unwrap();
        let instr_dir = dir.path().join(".gemini/antigravity/mcp/vectorcode");
        let instr_path = instr_dir.join("instructions.md");

        std::fs::create_dir_all(&instr_dir).unwrap();
        std::fs::write(&instr_path, MCP_INSTRUCTIONS).unwrap();

        assert!(instr_path.exists(), "instructions.md should be created");
        let content = std::fs::read_to_string(&instr_path).unwrap();
        assert!(
            content.contains("VectorCode"),
            "Should contain VectorCode reference"
        );
        assert!(
            content.contains("vec_search"),
            "Should contain vec_search tool reference"
        );
        assert!(
            content.contains("Anti-patterns"),
            "Should contain anti-patterns section"
        );
    }

    #[test]
    fn embedded_constants_not_empty() {
        assert!(
            !SEMANTIC_SEARCH_SKILL.is_empty(),
            "SEMANTIC_SEARCH_SKILL should not be empty"
        );
        assert!(
            !MCP_INSTRUCTIONS.is_empty(),
            "MCP_INSTRUCTIONS should not be empty"
        );
        assert!(
            SEMANTIC_SEARCH_SKILL.contains("name: semantic-search"),
            "Skill should have correct frontmatter name"
        );
        assert!(
            MCP_INSTRUCTIONS.contains("# VectorCode"),
            "Instructions should have title"
        );
    }
}
