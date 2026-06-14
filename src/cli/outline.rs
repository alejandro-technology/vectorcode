//! `vectorcode outline` — show outline of a source file's top-level symbols.

use crate::engine::languages::SupportedLanguage;
use crate::engine::outliner;
use anyhow::Result;
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
    let requested_path = project_path.join(&args.file_path);
    let canonical = std::fs::canonicalize(&requested_path)?;
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
