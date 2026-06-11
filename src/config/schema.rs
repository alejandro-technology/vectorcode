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
}

/// Provider selection and per-provider settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    /// Active provider: "onnx" | "gemini" | "ollama" | "openai"
    pub name: String,
    pub onnx: Option<OnnxConfig>,
    pub gemini: Option<GeminiConfig>,
    pub ollama: Option<OllamaConfig>,
    pub openai: Option<OpenAiConfig>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            name: "onnx".to_string(),
            onnx: None,
            gemini: None,
            ollama: None,
            openai: None,
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
    pub api_key: String,
    pub model: String,
    pub dimensions: u32,
}

impl Default for GeminiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "gemini-embedding-001".to_string(),
            dimensions: 768,
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
            model: "nomic-embed-text".to_string(),
        }
    }
}

/// OpenAI provider settings.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct OpenAiConfig {
    pub api_key: String,
    pub model: String,
}

impl Default for OpenAiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            model: "text-embedding-3-small".to_string(),
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
            ],
            concurrency: 8,
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
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            default_limit: 10,
            default_threshold: 0.3,
        }
    }
}
