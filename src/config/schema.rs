use serde::Deserialize;

/// Top-level configuration matching spec §13.2.
///
/// Loaded from `.vectorcode/config.toml` with env var overrides applied
/// by `load_config()` in the parent module.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    pub provider: ProviderConfig,
    pub indexing: IndexingConfig,
    pub watcher: WatcherConfig,
    pub search: SearchConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            provider: ProviderConfig {
                name: "onnx".to_string(),
                onnx: None,
                gemini: None,
                ollama: None,
                openai: None,
                openrouter: None,
            },
            indexing: IndexingConfig::default(),
            watcher: WatcherConfig::default(),
            search: SearchConfig::default(),
        }
    }
}

impl Config {
    /// Apply environment variable overrides per spec §13.3.
    ///
    /// Priority: env vars override file values. Called by `load_config()`
    /// after deserializing the TOML file.
    pub fn apply_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("VECTORCODE_PROVIDER") {
            self.provider.name = val;
        }
        if let Ok(val) = std::env::var("GEMINI_API_KEY") {
            self.provider
                .gemini
                .get_or_insert_with(GeminiConfig::default)
                .api_key = val;
        }
        if let Ok(val) = std::env::var("OPENAI_API_KEY") {
            self.provider
                .openai
                .get_or_insert_with(OpenAiConfig::default)
                .api_key = val;
        }
        if let Ok(val) = std::env::var("OPENROUTER_API_KEY") {
            self.provider
                .openrouter
                .get_or_insert_with(OpenRouterConfig::default)
                .api_key = val;
        }
        if let Ok(val) = std::env::var("VECTORCODE_NO_WATCH") {
            if val == "1" {
                self.watcher.disabled = true;
            }
        }
        if let Ok(val) = std::env::var("VECTORCODE_DEBOUNCE_MS") {
            if let Ok(ms) = val.parse::<u64>() {
                self.watcher.debounce_ms = ms;
            }
        }
    }

    /// Validate configuration bounds and requirements.
    pub fn validate(&self) -> Result<(), String> {
        let valid_providers = ["onnx", "gemini", "ollama", "openai", "openrouter", "mock"];
        if !valid_providers.contains(&self.provider.name.as_str()) {
            return Err(format!("Unknown provider: {}", self.provider.name));
        }

        if self.indexing.max_file_size == 0 {
            return Err("max_file_size must be greater than 0".to_string());
        }

        if self.indexing.concurrency == 0 {
            return Err("concurrency must be greater than 0".to_string());
        }

        if self.indexing.chunk_overlap >= 50 {
            return Err("chunk_overlap must be less than 50".to_string());
        }

        if self.watcher.debounce_ms == 0 {
            return Err("watcher debounce_ms must be greater than 0".to_string());
        }

        if self.search.default_limit == 0 {
            return Err("search default_limit must be greater than 0".to_string());
        }

        if self.search.default_threshold < 0.0 || self.search.default_threshold > 1.0 {
            return Err(format!(
                "search default_threshold must be between 0.0 and 1.0, got {}",
                self.search.default_threshold
            ));
        }

        let valid_modes = ["dense", "sparse", "hybrid"];
        if !valid_modes.contains(&self.search.default_mode.as_str()) {
            return Err(format!(
                "search default_mode must be one of: dense, sparse, hybrid. Got: {}",
                self.search.default_mode
            ));
        }

        // Validate active provider specific fields
        match self.provider.name.as_str() {
            "gemini" => {
                if let Some(gemini) = &self.provider.gemini {
                    if (gemini.api_key.trim().is_empty() || gemini.api_key == "your-api-key")
                        && !gemini.api_key_from_env
                    {
                        return Err(
                            "Gemini API key is empty or not configured. Set GEMINI_API_KEY env var or run `vectorcode init`.".to_string()
                        );
                    }
                    if gemini.dimensions == 0 {
                        return Err("Gemini dimensions must be greater than 0".to_string());
                    }
                } else {
                    return Err("Gemini config section is missing".to_string());
                }
            }
            "openai" => {
                if let Some(openai) = &self.provider.openai {
                    if (openai.api_key.trim().is_empty() || openai.api_key == "your-api-key")
                        && !openai.api_key_from_env
                    {
                        return Err(
                            "OpenAI API key is empty or not configured. Set OPENAI_API_KEY env var or run `vectorcode init`.".to_string()
                        );
                    }
                } else {
                    return Err("OpenAI config section is missing".to_string());
                }
            }
            "openrouter" => {
                if let Some(openrouter) = &self.provider.openrouter {
                    if (openrouter.api_key.trim().is_empty()
                        || openrouter.api_key == "your-api-key")
                        && !openrouter.api_key_from_env
                    {
                        return Err(
                            "OpenRouter API key is empty or not configured. Set OPENROUTER_API_KEY env var or run `vectorcode init`.".to_string()
                        );
                    }
                    if openrouter.dimensions == 0 {
                        return Err("OpenRouter dimensions must be greater than 0".to_string());
                    }
                } else {
                    return Err("OpenRouter config section is missing".to_string());
                }
            }
            "ollama" => {
                if let Some(ollama) = &self.provider.ollama {
                    if ollama.url.trim().is_empty() {
                        return Err("Ollama URL is empty".to_string());
                    }
                } else {
                    return Err("Ollama config section is missing".to_string());
                }
            }
            _ => {}
        }

        Ok(())
    }
}

/// Provider selection and per-provider settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Active provider: "onnx" | "gemini" | "ollama" | "openai" | "openrouter"
    pub name: String,
    pub onnx: Option<OnnxConfig>,
    pub gemini: Option<GeminiConfig>,
    pub ollama: Option<OllamaConfig>,
    pub openai: Option<OpenAiConfig>,
    pub openrouter: Option<OpenRouterConfig>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: "onnx".to_string(),
            onnx: None,
            gemini: None,
            ollama: None,
            openai: None,
            openrouter: None,
        }
    }
}

/// ONNX provider — model is bundled, no user configuration needed.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OnnxConfig {
    pub model: String,
}

impl Default for OnnxConfig {
    fn default() -> Self {
        Self {
            model: "all-MiniLM-L6-v2".to_string(),
        }
    }
}

/// Gemini provider settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GeminiConfig {
    #[serde(default)]
    pub api_key: String,
    pub model: String,
    pub dimensions: u32,
    /// If true, the API key is loaded from `.vectorcode/.env` instead of this file.
    #[serde(default)]
    pub api_key_from_env: bool,
}

impl Default for GeminiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "gemini-embedding-001".to_string(),
            dimensions: 768,
            api_key_from_env: false,
        }
    }
}

/// Ollama provider settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OllamaConfig {
    pub url: String,
    pub model: String,
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:11434".to_string(),
            model: "embeddinggemma:latest".to_string(),
        }
    }
}

/// OpenAI provider settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OpenAiConfig {
    #[serde(default)]
    pub api_key: String,
    pub model: String,
    /// If true, the API key is loaded from `.vectorcode/.env` instead of this file.
    #[serde(default)]
    pub api_key_from_env: bool,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "text-embedding-3-small".to_string(),
            api_key_from_env: false,
        }
    }
}

/// OpenRouter provider settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OpenRouterConfig {
    #[serde(default)]
    pub api_key: String,
    pub model: String,
    pub dimensions: u32,
    /// If true, the API key is loaded from `.vectorcode/.env` instead of this file.
    #[serde(default)]
    pub api_key_from_env: bool,
}

impl Default for OpenRouterConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "nvidia/llama-nemotron-embed-vl-1b-v2:free".to_string(),
            dimensions: 768,
            api_key_from_env: false,
        }
    }
}

/// Indexing pipeline settings per spec §13.2.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct IndexingConfig {
    /// Maximum file size in bytes (default: 1MB).
    pub max_file_size: u64,
    /// Directories to always exclude.
    pub exclude_dirs: Vec<String>,
    /// File extensions to always exclude.
    pub exclude_extensions: Vec<String>,
    /// Max concurrent file processing tasks.
    pub concurrency: usize,
    /// Line overlap for chunking.
    pub chunk_overlap: usize,
}

impl Default for IndexingConfig {
    fn default() -> Self {
        Self {
            max_file_size: 1_048_576,
            exclude_dirs: vec![
                ".vectorcode".into(),
                ".git".into(),
                "node_modules".into(),
                "target".into(),
                "__pycache__".into(),
                "vendor".into(),
                "dist".into(),
                "build".into(),
                ".next".into(),
                "benchmarks".into(),
                "fixtures".into(),
                "tests".into(),
            ],
            exclude_extensions: vec![
                ".min.js".into(),
                ".map".into(),
                ".lock".into(),
                ".svg".into(),
                ".png".into(),
                ".jpg".into(),
                ".ico".into(),
                ".woff".into(),
                ".woff2".into(),
                ".ttf".into(),
                ".json".into(),
                ".txt".into(),
                ".md".into(),
                ".toml".into(),
                ".yaml".into(),
                ".yml".into(),
                ".sh".into(),
            ],
            concurrency: 8,
            chunk_overlap: 10,
        }
    }
}

/// File watcher settings per spec §13.2.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct WatcherConfig {
    /// Debounce window in milliseconds.
    pub debounce_ms: u64,
    /// Disable file watcher entirely.
    pub disabled: bool,
}

impl Default for WatcherConfig {
    fn default() -> Self {
        Self {
            debounce_ms: 2000,
            disabled: false,
        }
    }
}

/// Search defaults per spec §13.2.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    /// Default maximum number of results.
    pub default_limit: usize,
    /// Default minimum similarity threshold (0.0–1.0).
    pub default_threshold: f32,
    /// Default search mode: "dense", "sparse", or "hybrid".
    pub default_mode: String,
    /// RRF K parameter for reciprocal rank fusion (default: 60).
    pub rrf_k: u32,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            default_limit: 10,
            default_threshold: 0.3,
            default_mode: "dense".to_string(),
            rrf_k: 60,
        }
    }
}

impl SearchConfig {
    /// Parse `default_mode` string into a `SearchMode` enum.
    ///
    /// Falls back to `SearchMode::Dense` if the string is invalid.
    pub fn search_mode(&self) -> crate::engine::SearchMode {
        self.default_mode
            .parse()
            .unwrap_or(crate::engine::SearchMode::Dense)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── SearchConfig default values ───────────────────────────────────

    #[test]
    fn search_config_default_mode_is_dense() {
        let config = SearchConfig::default();
        assert_eq!(config.default_mode, "dense");
    }

    #[test]
    fn search_config_default_rrf_k_is_60() {
        let config = SearchConfig::default();
        assert_eq!(config.rrf_k, 60);
    }

    #[test]
    fn search_config_default_preserves_existing_fields() {
        let config = SearchConfig::default();
        assert_eq!(config.default_limit, 10);
        assert!((config.default_threshold - 0.3).abs() < f32::EPSILON);
    }

    // ─── SearchConfig::search_mode() ───────────────────────────────────

    #[test]
    fn search_config_search_mode_parses_dense() {
        let config = SearchConfig::default();
        assert_eq!(config.search_mode(), crate::engine::SearchMode::Dense);
    }

    #[test]
    fn search_config_search_mode_parses_sparse() {
        let config = SearchConfig {
            default_mode: "sparse".to_string(),
            ..Default::default()
        };
        assert_eq!(config.search_mode(), crate::engine::SearchMode::Sparse);
    }

    #[test]
    fn search_config_search_mode_parses_hybrid() {
        let config = SearchConfig {
            default_mode: "hybrid".to_string(),
            ..Default::default()
        };
        assert_eq!(config.search_mode(), crate::engine::SearchMode::Hybrid);
    }

    #[test]
    fn search_config_search_mode_invalid_falls_back_to_dense() {
        let config = SearchConfig {
            default_mode: "invalid".to_string(),
            ..Default::default()
        };
        assert_eq!(config.search_mode(), crate::engine::SearchMode::Dense);
    }

    // ─── SearchConfig TOML deserialization ─────────────────────────────

    #[test]
    fn search_config_deserialize_with_new_fields() {
        let toml_str = r#"
            default_limit = 20
            default_threshold = 0.5
            default_mode = "hybrid"
            rrf_k = 100
        "#;
        let config: SearchConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default_limit, 20);
        assert!((config.default_threshold - 0.5).abs() < f32::EPSILON);
        assert_eq!(config.default_mode, "hybrid");
        assert_eq!(config.rrf_k, 100);
    }

    #[test]
    fn search_config_deserialize_defaults_when_missing() {
        let toml_str = r#"
            default_limit = 5
            default_threshold = 0.1
        "#;
        let config: SearchConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.default_limit, 5);
        assert!((config.default_threshold - 0.1).abs() < f32::EPSILON);
        assert_eq!(config.default_mode, "dense");
        assert_eq!(config.rrf_k, 60);
    }

    // ─── Config::validate() for default_mode ───────────────────────────

    #[test]
    fn validate_rejects_invalid_search_mode() {
        let mut config = Config::default();
        config.search.default_mode = "bogus".to_string();
        let result = config.validate();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("default_mode"));
    }

    #[test]
    fn validate_accepts_valid_search_modes() {
        for mode in &["dense", "sparse", "hybrid"] {
            let mut config = Config::default();
            config.search.default_mode = mode.to_string();
            assert!(config.validate().is_ok(), "Mode '{mode}' should be valid");
        }
    }
}
