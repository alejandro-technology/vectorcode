//! `vectorcode upgrade` — self-update the binary.

use anyhow::Result;
use clap::Args;

/// Arguments for `vectorcode upgrade`.
#[derive(Args, Debug)]
pub struct UpgradeArgs {
    /// Check for updates without installing.
    #[arg(long)]
    pub check: bool,
}

/// Execute the `upgrade` command.
///
/// Stub implementation — prints that self-update is not yet available.
pub fn execute(args: &UpgradeArgs) -> Result<()> {
    if args.check {
        eprintln!("Current version: {}", env!("CARGO_PKG_VERSION"));
        eprintln!("Check for updates: not yet implemented");
        eprintln!("Visit https://github.com/your-org/vectorcode/releases for latest version.");
    } else {
        eprintln!("Self-update is not yet implemented.");
        eprintln!("To upgrade, download the latest binary from:");
        eprintln!("  https://github.com/your-org/vectorcode/releases");
        eprintln!();
        eprintln!("Or reinstall via:");
        eprintln!("  curl -fsSL https://raw.githubusercontent.com/your-org/vectorcode/main/install.sh | sh");
    }

    // TODO: Real implementation would:
    // 1. Check GitHub releases API for latest version
    // 2. Compare with current version
    // 3. Download the appropriate binary for the platform
    // 4. Replace the current binary (atomic swap)

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::Parser;

    #[test]
    fn upgrade_args_parse_defaults() {
        let cli = Cli::parse_from(["vectorcode", "upgrade"]);
        match cli.command {
            crate::cli::Commands::Upgrade(args) => {
                assert!(!args.check);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn upgrade_args_parse_check_flag() {
        let cli = Cli::parse_from(["vectorcode", "upgrade", "--check"]);
        match cli.command {
            crate::cli::Commands::Upgrade(args) => {
                assert!(args.check);
            }
            _ => panic!("Expected Upgrade command"),
        }
    }

    #[test]
    fn upgrade_execute_succeeds() {
        let args = UpgradeArgs { check: false };
        let result = execute(&args);
        assert!(result.is_ok(), "Upgrade stub should succeed");
    }

    #[test]
    fn upgrade_execute_check_succeeds() {
        let args = UpgradeArgs { check: true };
        let result = execute(&args);
        assert!(result.is_ok());
    }
}
