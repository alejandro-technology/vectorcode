//! `vectorcode outline` — show outline of a source file's top-level symbols.

use crate::engine::languages::SupportedLanguage;
use crate::engine::outliner;
use crate::mcp::security::resolve_within_project;
use anyhow::{bail, Result};
use clap::Args;
use std::path::Path;

/// Arguments for `vectorcode outline`.
#[derive(Args, Debug)]
pub struct OutlineArgs {
    /// The file path to outline (relative to project root).
    pub file_path: String,
}

/// Execute the `outline` command.
pub fn execute(args: &OutlineArgs, project_path: &Path) -> Result<()> {
    // REQ-SEC-04: reject paths that fall outside the project root.
    let canonical = match resolve_within_project(&args.file_path, project_path) {
        Ok(p) => p,
        Err(e) => {
            bail!(
                "Refusing to outline '{}': {} (path is outside the project root)",
                args.file_path,
                e
            );
        }
    };
    let source = std::fs::read_to_string(&canonical)?;
    let ext = canonical.extension().and_then(|e| e.to_str()).unwrap_or("");
    let language = SupportedLanguage::from_extension(ext);
    let items = outliner::outline_file(&source, &args.file_path, language);

    if items.is_empty() {
        println!("No outline items found.");
        return Ok(());
    }

    println!("Outline of {} ({} items):\n", args.file_path, items.len());
    for item in &items {
        let vis = item
            .visibility
            .as_deref()
            .map(|v| format!("{v} "))
            .unwrap_or_default();
        println!(
            "  L{:<5} {}{} {}",
            item.start_line, vis, item.kind, item.signature
        );
    }
    Ok(())
}
