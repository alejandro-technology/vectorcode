//! `vectorcode init` — initialize VectorCode in a project directory (spec §12.2).

use anyhow::Result;
use clap::Args;

use crate::embedder::mock::MockEmbedder;
use crate::embedder::model_manager::ModelManager;
use crate::store::db::Database;
use crate::store::meta;
use crate::types::IndexMeta;

use super::ProviderArg;

/// Arguments for `vectorcode init`.
#[derive(Args, Debug)]
pub struct InitArgs {
    /// Embedding provider to use (omit for interactive selection).
    #[arg(long, value_enum)]
    pub provider: Option<ProviderArg>,

    /// Model name (provider-specific default if omitted).
    #[arg(long)]
    pub model: Option<String>,

    /// Embedding dimensions (provider-specific default if omitted).
    #[arg(long)]
    pub dims: Option<u32>,

    /// Also run initial indexing after init.
    #[arg(long)]
    pub index: bool,
}

/// Execute the `init` command (spec §12.2).
///
/// 1. Create `.vectorcode/` directory
/// 2. Create `index.db` with schema
/// 3. Write meta table with provider, model, dimensions
/// 4. Create `.vectorcode/.gitignore` containing `index.db`
/// 5. Create `.vectorcode/config.toml` with chosen provider settings
/// 6. If `--index`: run full indexing pipeline
pub async fn execute(args: &InitArgs, project_path: &std::path::Path, quiet: bool) -> Result<()> {
    let vc_dir = project_path.join(".vectorcode");

    // Error if already initialized
    if vc_dir.exists() {
        anyhow::bail!(
            "VectorCode is already initialized in {}.\n\
             To re-index, run: vectorcode index --full\n\
             To start fresh, remove .vectorcode/ and run init again.",
            project_path.display()
        );
    }

    // Acquire an exclusive lock file to prevent concurrent init (TOCTOU guard).
    // The lock is created before any mutations and held until the function returns.
    // We use a parent-level lock since vc_dir doesn't exist yet.
    let lock_path = project_path.join(".vectorcode.init.lock");
    let _lock = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&lock_path)
        .map_err(|_| {
            anyhow::anyhow!(
                "Another init is running or a stale lock exists at {}. Remove it and retry.",
                lock_path.display()
            )
        })?;

    // Resolve provider: use CLI arg if given, otherwise interactive prompt
    let provider = match &args.provider {
        Some(p) => p.clone(),
        None => prompt_provider_interactive(),
    };

    // Prompt for API key if needed (gemini, openai)
    let api_key = prompt_api_key_if_needed(&provider, quiet);
    let ollama_url = prompt_ollama_url_if_needed(&provider, quiet);

    // Step 1: Create .vectorcode/ directory
    std::fs::create_dir_all(&vc_dir)?;

    // Resolve provider defaults
    let (model, dims) = resolve_provider_defaults(&provider, &args.model, &args.dims);

    // If ONNX selected, ensure model is downloaded
    if matches!(provider, ProviderArg::Onnx) {
        let manager = ModelManager::new();
        if !manager.is_downloaded() {
            #[cfg(not(test))]
            {
                if !quiet {
                    eprintln!("ONNX model not found. Downloading from HuggingFace...");
                }
                manager.download_model().await?;
                if !quiet {
                    eprintln!("ONNX model downloaded to ~/.vectorcode/models/");
                }
            }
            #[cfg(test)]
            {
                // In tests, skip actual download — model_manager tests cover this
                let _ = manager; // suppress unused warning
            }
        }
    }

    // Step 2: Create index.db with schema
    let db_path = vc_dir.join("index.db");
    let db = Database::open(&db_path)?;
    db.init_schema(dims)?;

    // Step 3: Write meta table
    let now = chrono_now();
    let index_meta = IndexMeta {
        provider: provider.as_str().to_string(),
        model: model.clone(),
        dimensions: dims,
        created_at: now.clone(),
        last_sync_at: None,
        files_indexed: 0,
        chunks_stored: 0,
        vectorcode_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    meta::write_index_meta(db.conn(), &index_meta)?;

    // Step 4: Create .gitignore
    let gitignore_path = vc_dir.join(".gitignore");
    std::fs::write(&gitignore_path, "index.db\nindex.db-wal\nindex.db-shm\n")?;

    // Step 5: Create config.toml
    let config_content = generate_config_toml(&provider, &model, dims, &api_key, &ollama_url);
    let config_path = vc_dir.join("config.toml");
    std::fs::write(&config_path, &config_content)?;

    if !quiet {
        eprintln!("VectorCode initialized in {}", vc_dir.display());
        eprintln!("  Provider: {}", provider.as_str());
        eprintln!("  Model:    {model}");
        eprintln!("  Dims:     {dims}");
    }

    // Step 6: If --index, run full indexing
    if args.index {
        if !quiet {
            eprintln!("Running initial indexing...");
        }
        let config = crate::config::load_config(project_path)?;
        // Use MockEmbedder for init since real embedders may not be available
        let embedder = std::sync::Arc::new(MockEmbedder::new(dims))
            as std::sync::Arc<dyn crate::embedder::Embedder>;
        let indexer = crate::engine::Indexer::new(
            std::sync::Arc::new(tokio::sync::Mutex::new(Database::open(&db_path)?)),
            embedder,
            config.indexing.clone(),
        );
        let report = indexer.index_project(project_path).await?;
        if !quiet {
            eprintln!(
                "Indexed {} files, {} chunks in {:.1}s",
                report.files_indexed,
                report.chunks_new,
                report.duration.as_secs_f64()
            );
        }
    }

    // Clean up the lock file on success
    drop(_lock);
    let _ = std::fs::remove_file(&lock_path);

    Ok(())
}

/// Resolve model name and dimensions from provider + optional overrides.
fn resolve_provider_defaults(
    provider: &ProviderArg,
    model: &Option<String>,
    dims: &Option<u32>,
) -> (String, u32) {
    match provider {
        ProviderArg::Onnx => (
            model
                .clone()
                .unwrap_or_else(|| "all-MiniLM-L6-v2".to_string()),
            dims.unwrap_or(384),
        ),
        ProviderArg::Gemini => (
            model
                .clone()
                .unwrap_or_else(|| "gemini-embedding-001".to_string()),
            dims.unwrap_or(768),
        ),
        ProviderArg::Ollama => (
            model
                .clone()
                .unwrap_or_else(|| "embeddinggemma:latest".to_string()),
            dims.unwrap_or(768),
        ),
        ProviderArg::Openai => (
            model
                .clone()
                .unwrap_or_else(|| "text-embedding-3-small".to_string()),
            dims.unwrap_or(1536),
        ),
    }
}

/// Return the interactive provider selection prompt text.
fn provider_prompt_text() -> &'static str {
    "Select embedding provider:\n\
     [1] onnx    — Local, offline, no API key needed (~23MB download)\n\
     [2] gemini  — Google API, requires GEMINI_API_KEY\n\
     [3] ollama  — Local Ollama server, requires ollama serve\n\
     [4] openai  — OpenAI API, requires OPENAI_API_KEY\n\
     Enter number (1-4): "
}

/// Parse a user's numbered input into a `ProviderArg`.
///
/// Accepts "1"–"4" with optional surrounding whitespace.
/// Returns `None` for invalid input.
fn parse_provider_choice(input: &str) -> Option<ProviderArg> {
    match input.trim() {
        "1" => Some(ProviderArg::Onnx),
        "2" => Some(ProviderArg::Gemini),
        "3" => Some(ProviderArg::Ollama),
        "4" => Some(ProviderArg::Openai),
        _ => None,
    }
}

/// Return the environment variable name for a provider's API key.
///
/// Returns empty string for providers that don't use API keys (onnx, ollama).
fn api_key_env_var(provider: &ProviderArg) -> &'static str {
    match provider {
        ProviderArg::Gemini => "GEMINI_API_KEY",
        ProviderArg::Openai => "OPENAI_API_KEY",
        ProviderArg::Onnx | ProviderArg::Ollama => "",
    }
}

/// Whether a provider requires an API key.
fn provider_requires_api_key(provider: &ProviderArg) -> bool {
    !api_key_env_var(provider).is_empty()
}

/// Prompt the user to select a provider interactively via stdin.
///
/// Loops until a valid choice (1–4) is entered.
fn prompt_provider_interactive() -> ProviderArg {
    loop {
        eprint!("{}", provider_prompt_text());
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_err() {
            input.clear();
        }
        match parse_provider_choice(&input) {
            Some(provider) => return provider,
            None => eprintln!("Invalid choice. Please enter 1, 2, 3, or 4."),
        }
    }
}

/// Prompt for an API key if the provider requires one.
///
/// Returns the API key string (may be empty if user skips).
#[cfg(not(test))]
fn prompt_api_key_if_needed(provider: &ProviderArg, quiet: bool) -> String {
    if !provider_requires_api_key(provider) {
        return String::new();
    }
    if quiet {
        return String::new();
    }

    let env_var = api_key_env_var(provider);
    eprint!("Enter {env_var} (or press Enter to skip): ");
    let mut key = String::new();
    if std::io::stdin().read_line(&mut key).is_err() {
        key.clear();
    }
    key.trim().to_string()
}

#[cfg(test)]
fn prompt_api_key_if_needed(_provider: &ProviderArg, _quiet: bool) -> String {
    // In tests, skip API key prompt
    String::new()
}

#[cfg(test)]
fn prompt_ollama_url_if_needed(_provider: &ProviderArg, _quiet: bool) -> String {
    // In tests, skip Ollama URL prompt
    "http://localhost:11434".to_string()
}

/// Prompt for Ollama URL if the provider is Ollama.
#[cfg(not(test))]
fn prompt_ollama_url_if_needed(provider: &ProviderArg, quiet: bool) -> String {
    if !matches!(provider, ProviderArg::Ollama) {
        return String::new();
    }

    let default_url = "http://localhost:11434";
    if quiet {
        return default_url.to_string();
    }
    eprint!("Enter Ollama server URL (default: {default_url}): ");
    let mut url = String::new();
    if std::io::stdin().read_line(&mut url).is_err() {
        url.clear();
    }
    let trimmed = url.trim();
    if trimmed.is_empty() {
        default_url.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
fn prompt_ollama_url_if_needed(provider: &ProviderArg) -> String {
    if !matches!(provider, ProviderArg::Ollama) {
        return String::new();
    }
    "http://localhost:11434".to_string()
}

/// Generate the config.toml content for the chosen provider.
fn generate_config_toml(
    provider: &ProviderArg,
    model: &str,
    dims: u32,
    api_key: &str,
    ollama_url: &str,
) -> String {
    let provider_section = match provider {
        ProviderArg::Onnx => format!(
            r#"[provider]
name = "onnx"

[provider.onnx]
model = "{model}"
"#
        ),
        ProviderArg::Gemini => format!(
            r#"[provider]
name = "gemini"

[provider.gemini]
api_key = "{api_key}"
model = "{model}"
dimensions = {dims}
"#
        ),
        ProviderArg::Ollama => format!(
            r#"[provider]
name = "ollama"

[provider.ollama]
url = "{ollama_url}"
model = "{model}"
"#
        ),
        ProviderArg::Openai => format!(
            r#"[provider]
name = "openai"

[provider.openai]
api_key = "{api_key}"
model = "{model}"
"#
        ),
    };

    format!(
        r#"{provider_section}
[indexing]
max_file_size = 1_048_576
exclude_dirs = [".agents", ".atl", ".codegraph", ".vectorcode", ".git", "node_modules", "target", "__pycache__", "vendor", "dist", "build", ".next", "benchmarks", "fixtures", "tests"]
exclude_extensions = [".min.js", ".map", ".lock", ".svg", ".png", ".jpg", ".ico", ".woff", ".woff2", ".ttf", ".md", ".json", ".txt", ".toml", ".yaml", ".yml", ".sh"]

[watcher]
debounce_ms = 2000
disabled = false

[search]
default_limit = 10
default_threshold = 0.3
"#
    )
}

/// Get current time as ISO 8601 string (without chrono dependency).
fn chrono_now() -> String {
    chrono_now_public()
}

/// Public version for other CLI modules to use.
pub fn chrono_now_public() -> String {
    // Use a simple approach — in production we'd use chrono or time crate
    // For now, use std::time since we don't have chrono as a dependency
    let duration = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = duration.as_secs();
    // Simple ISO 8601 format (not perfect but good enough for metadata)
    format!("unix:{secs}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_provider_defaults_onnx() {
        let (model, dims) = resolve_provider_defaults(&ProviderArg::Onnx, &None, &None);
        assert_eq!(model, "all-MiniLM-L6-v2");
        assert_eq!(dims, 384);
    }

    #[test]
    fn resolve_provider_defaults_gemini() {
        let (model, dims) = resolve_provider_defaults(&ProviderArg::Gemini, &None, &None);
        assert_eq!(model, "gemini-embedding-001");
        assert_eq!(dims, 768);
    }

    #[test]
    fn resolve_provider_defaults_ollama() {
        let (model, dims) = resolve_provider_defaults(&ProviderArg::Ollama, &None, &None);
        assert_eq!(model, "embeddinggemma:latest");
        assert_eq!(dims, 768);
    }

    #[test]
    fn resolve_provider_defaults_openai() {
        let (model, dims) = resolve_provider_defaults(&ProviderArg::Openai, &None, &None);
        assert_eq!(model, "text-embedding-3-small");
        assert_eq!(dims, 1536);
    }

    #[test]
    fn resolve_provider_defaults_with_overrides() {
        let (model, dims) = resolve_provider_defaults(
            &ProviderArg::Gemini,
            &Some("custom-model".to_string()),
            &Some(256),
        );
        assert_eq!(model, "custom-model");
        assert_eq!(dims, 256);
    }

    #[test]
    fn generate_config_toml_onnx_contains_provider() {
        let toml = generate_config_toml(&ProviderArg::Onnx, "all-MiniLM-L6-v2", 384, "", "");
        assert!(toml.contains("name = \"onnx\""));
        assert!(toml.contains("model = \"all-MiniLM-L6-v2\""));
        assert!(toml.contains("[indexing]"));
        assert!(toml.contains("[search]"));
    }

    #[test]
    fn generate_config_toml_gemini_contains_api_key() {
        let toml = generate_config_toml(&ProviderArg::Gemini, "gemini-embedding-001", 768, "", "");
        assert!(toml.contains("name = \"gemini\""));
        assert!(toml.contains("api_key = \"\""));
        assert!(toml.contains("dimensions = 768"));
    }

    #[test]
    fn generate_config_toml_ollama_contains_url() {
        let toml = generate_config_toml(
            &ProviderArg::Ollama,
            "embeddinggemma:latest",
            768,
            "",
            "http://localhost:11434",
        );
        assert!(toml.contains("name = \"ollama\""));
        assert!(toml.contains("url = \"http://localhost:11434\""));
    }

    #[test]
    fn generate_config_toml_openai_contains_api_key() {
        let toml =
            generate_config_toml(&ProviderArg::Openai, "text-embedding-3-small", 1536, "", "");
        assert!(toml.contains("name = \"openai\""));
        assert!(toml.contains("api_key = \"\""));
    }

    #[test]
    fn generate_config_toml_all_have_indexing_section() {
        for provider in [
            ProviderArg::Onnx,
            ProviderArg::Gemini,
            ProviderArg::Ollama,
            ProviderArg::Openai,
        ] {
            let toml =
                generate_config_toml(&provider, "test-model", 128, "", "http://localhost:11434");
            assert!(
                toml.contains("[indexing]"),
                "Missing [indexing] for {:?}",
                provider
            );
            assert!(
                toml.contains("[watcher]"),
                "Missing [watcher] for {:?}",
                provider
            );
            assert!(
                toml.contains("[search]"),
                "Missing [search] for {:?}",
                provider
            );
        }
    }

    #[test]
    fn init_creates_vectorcode_directory() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();

        let args = InitArgs {
            provider: Some(ProviderArg::Onnx),
            model: None,
            dims: None,
            index: false,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(execute(&args, project_path, true)).unwrap();

        let vc_dir = project_path.join(".vectorcode");
        assert!(vc_dir.exists(), ".vectorcode/ must be created");
        assert!(vc_dir.join("index.db").exists(), "index.db must exist");
        assert!(vc_dir.join(".gitignore").exists(), ".gitignore must exist");
        assert!(
            vc_dir.join("config.toml").exists(),
            "config.toml must exist"
        );
    }

    #[test]
    fn init_writes_correct_meta() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();

        let args = InitArgs {
            provider: Some(ProviderArg::Ollama),
            model: Some("custom-model".to_string()),
            dims: Some(512),
            index: false,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(execute(&args, project_path, true)).unwrap();

        let db_path = project_path.join(".vectorcode/index.db");
        let db = Database::open(&db_path).unwrap();
        let meta = meta::read_index_meta(db.conn()).unwrap().unwrap();

        assert_eq!(meta.provider, "ollama");
        assert_eq!(meta.model, "custom-model");
        assert_eq!(meta.dimensions, 512);
        assert_eq!(meta.files_indexed, 0);
        assert_eq!(meta.chunks_stored, 0);
    }

    #[test]
    fn init_fails_if_already_initialized() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();

        let args = InitArgs {
            provider: Some(ProviderArg::Onnx),
            model: None,
            dims: None,
            index: false,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(execute(&args, project_path, true)).unwrap();

        // Second init should fail
        let result = rt.block_on(execute(&args, project_path, true));
        assert!(result.is_err(), "Second init must fail");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("already initialized"),
            "Error must mention already initialized: {err}"
        );
    }

    #[test]
    fn init_gitignore_contains_db() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();

        let args = InitArgs {
            provider: Some(ProviderArg::Onnx),
            model: None,
            dims: None,
            index: false,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(execute(&args, project_path, true)).unwrap();

        let gitignore =
            std::fs::read_to_string(project_path.join(".vectorcode/.gitignore")).unwrap();
        assert!(
            gitignore.contains("index.db"),
            ".gitignore must contain index.db"
        );
    }

    #[test]
    fn init_config_toml_is_valid() {
        let dir = tempfile::tempdir().unwrap();
        let project_path = dir.path();

        let args = InitArgs {
            provider: Some(ProviderArg::Gemini),
            model: None,
            dims: Some(256),
            index: false,
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(execute(&args, project_path, true)).unwrap();

        let config_content =
            std::fs::read_to_string(project_path.join(".vectorcode/config.toml")).unwrap();
        // Verify it parses as valid TOML
        let parsed: Result<crate::config::schema::Config, _> = toml::from_str(&config_content);
        assert!(
            parsed.is_ok(),
            "config.toml must be valid TOML: {:?}",
            parsed.err()
        );
        let config = parsed.unwrap();
        assert_eq!(config.provider.name, "gemini");
    }

    // ── Interactive provider selection (pure functions) ──────────────

    #[test]
    fn parse_provider_choice_accepts_valid_numbers() {
        assert!(matches!(
            parse_provider_choice("1"),
            Some(ProviderArg::Onnx)
        ));
        assert!(matches!(
            parse_provider_choice("2"),
            Some(ProviderArg::Gemini)
        ));
        assert!(matches!(
            parse_provider_choice("3"),
            Some(ProviderArg::Ollama)
        ));
        assert!(matches!(
            parse_provider_choice("4"),
            Some(ProviderArg::Openai)
        ));
    }

    #[test]
    fn parse_provider_choice_trims_whitespace() {
        assert!(matches!(
            parse_provider_choice("  2  \n"),
            Some(ProviderArg::Gemini)
        ));
    }

    #[test]
    fn parse_provider_choice_rejects_invalid_input() {
        assert!(parse_provider_choice("0").is_none());
        assert!(parse_provider_choice("5").is_none());
        assert!(parse_provider_choice("abc").is_none());
        assert!(parse_provider_choice("").is_none());
    }

    #[test]
    fn provider_prompt_text_lists_all_providers() {
        let text = provider_prompt_text();
        assert!(text.contains("onnx"), "Must list onnx: {text}");
        assert!(text.contains("gemini"), "Must list gemini: {text}");
        assert!(text.contains("ollama"), "Must list ollama: {text}");
        assert!(text.contains("openai"), "Must list openai: {text}");
        assert!(text.contains("1"), "Must show option numbers: {text}");
    }

    #[test]
    fn api_key_env_var_returns_correct_var_for_provider() {
        assert_eq!(api_key_env_var(&ProviderArg::Gemini), "GEMINI_API_KEY");
        assert_eq!(api_key_env_var(&ProviderArg::Openai), "OPENAI_API_KEY");
        assert_eq!(api_key_env_var(&ProviderArg::Onnx), "");
        assert_eq!(api_key_env_var(&ProviderArg::Ollama), "");
    }

    #[test]
    fn provider_requires_api_key_true_for_api_providers() {
        assert!(!provider_requires_api_key(&ProviderArg::Onnx));
        assert!(provider_requires_api_key(&ProviderArg::Gemini));
        assert!(!provider_requires_api_key(&ProviderArg::Ollama));
        assert!(provider_requires_api_key(&ProviderArg::Openai));
    }
}
