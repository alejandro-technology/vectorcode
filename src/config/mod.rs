pub mod schema;

use anyhow::Result;
use schema::Config;

/// Load configuration with priority: env vars → config file → defaults.
///
/// `project_path` is the root of the project (where `.vectorcode/` lives).
pub fn load_config(project_path: &std::path::Path) -> Result<Config> {
    let config_path = project_path.join(".vectorcode").join("config.toml");

    let mut config = if config_path.exists() {
        let contents = std::fs::read_to_string(&config_path)?;
        let file_config: Config = toml::from_str(&contents)?;
        file_config
    } else {
        Config::default()
    };

    // Apply environment variable overrides
    config.apply_env_overrides();

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::*;
    use serial_test::serial;

    #[test]
    fn default_config_has_expected_values() {
        let cfg = Config::default();

        assert_eq!(cfg.provider.name, "onnx");
        assert_eq!(cfg.indexing.max_file_size, 1_048_576);
        assert_eq!(cfg.indexing.concurrency, 8);
        assert_eq!(cfg.watcher.debounce_ms, 2000);
        assert!(!cfg.watcher.disabled);
        assert_eq!(cfg.search.default_limit, 10);
        assert!((cfg.search.default_threshold - 0.3).abs() < f32::EPSILON);
    }

    #[test]
    fn parse_minimal_toml_uses_defaults_for_missing_sections() {
        let toml_str = r#"
[provider]
name = "gemini"
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.provider.name, "gemini");
        // Defaults for unspecified sections
        assert_eq!(cfg.indexing.concurrency, 8);
        assert_eq!(cfg.search.default_limit, 10);
    }

    #[test]
    fn parse_full_toml_roundtrip() {
        let toml_str = r#"
[provider]
name = "ollama"

[provider.ollama]
url = "http://custom:11434"
model = "mxbai-embed-large"

[indexing]
max_file_size = 2_097_152
concurrency = 4
exclude_dirs = ["custom_exclude"]

[watcher]
debounce_ms = 5000
disabled = true

[search]
default_limit = 20
default_threshold = 0.5
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        assert_eq!(cfg.provider.name, "ollama");
        assert_eq!(
            cfg.provider.ollama.as_ref().unwrap().url,
            "http://custom:11434"
        );
        assert_eq!(cfg.indexing.max_file_size, 2_097_152);
        assert_eq!(cfg.indexing.concurrency, 4);
        assert_eq!(cfg.indexing.exclude_dirs, vec!["custom_exclude"]);
        assert_eq!(cfg.watcher.debounce_ms, 5000);
        assert!(cfg.watcher.disabled);
        assert_eq!(cfg.search.default_limit, 20);
        assert!((cfg.search.default_threshold - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn gemini_provider_config_parses() {
        let toml_str = r#"
[provider]
name = "gemini"

[provider.gemini]
api_key = "test-key-123"
model = "gemini-embedding-001"
dimensions = 768
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        let gemini = cfg.provider.gemini.as_ref().unwrap();
        assert_eq!(gemini.api_key, "test-key-123");
        assert_eq!(gemini.model, "gemini-embedding-001");
        assert_eq!(gemini.dimensions, 768);
    }

    #[test]
    fn openai_provider_config_parses() {
        let toml_str = r#"
[provider]
name = "openai"

[provider.openai]
api_key = "sk-test"
model = "text-embedding-3-large"
"#;
        let cfg: Config = toml::from_str(toml_str).unwrap();
        let openai = cfg.provider.openai.as_ref().unwrap();
        assert_eq!(openai.api_key, "sk-test");
        assert_eq!(openai.model, "text-embedding-3-large");
    }

    #[test]
    #[serial]
    fn env_var_overrides_provider_name() {
        std::env::set_var("VECTORCODE_PROVIDER", "gemini");
        let mut cfg = Config::default();
        cfg.apply_env_overrides();
        assert_eq!(cfg.provider.name, "gemini");
        std::env::remove_var("VECTORCODE_PROVIDER");
    }

    #[test]
    #[serial]
    fn env_var_overrides_watcher_disabled() {
        std::env::set_var("VECTORCODE_NO_WATCH", "1");
        let mut cfg = Config::default();
        cfg.apply_env_overrides();
        assert!(cfg.watcher.disabled);
        std::env::remove_var("VECTORCODE_NO_WATCH");
    }

    #[test]
    #[serial]
    fn env_var_overrides_debounce_ms() {
        std::env::set_var("VECTORCODE_DEBOUNCE_MS", "5000");
        let mut cfg = Config::default();
        cfg.apply_env_overrides();
        assert_eq!(cfg.watcher.debounce_ms, 5000);
        std::env::remove_var("VECTORCODE_DEBOUNCE_MS");
    }

    #[test]
    #[serial]
    fn load_config_from_nonexistent_dir_returns_defaults() {
        // #[serial] ensures no other test has env vars set that would override defaults
        let dir = tempfile::tempdir().unwrap();
        let cfg = load_config(dir.path()).unwrap();
        assert_eq!(cfg.provider.name, "onnx");
        assert_eq!(cfg.indexing.concurrency, 8);
    }

    #[test]
    #[serial]
    fn load_config_reads_file_when_present() {
        let dir = tempfile::tempdir().unwrap();
        let vc_dir = dir.path().join(".vectorcode");
        std::fs::create_dir_all(&vc_dir).unwrap();
        std::fs::write(
            vc_dir.join("config.toml"),
            r#"
[provider]
name = "openai"

[search]
default_limit = 25
"#,
        )
        .unwrap();

        let cfg = load_config(dir.path()).unwrap();
        assert_eq!(cfg.provider.name, "openai");
        assert_eq!(cfg.search.default_limit, 25);
    }

    /// Regression test: load_config must not be affected by concurrent env var mutations.
    /// Uses #[serial] to guarantee no other env-touching test runs in parallel.
    #[test]
    #[serial]
    fn load_config_env_isolation_under_serial() {
        // Set an env var that would change the provider if read
        std::env::set_var("VECTORCODE_PROVIDER", "gemini");
        let dir = tempfile::tempdir().unwrap();
        let cfg = load_config(dir.path()).unwrap();
        // With #[serial], this test owns the env — override IS applied
        assert_eq!(cfg.provider.name, "gemini");
        // Cleanup
        std::env::remove_var("VECTORCODE_PROVIDER");
    }

    #[test]
    fn config_validation_rejects_invalid_values() {
        let mut cfg = Config::default();

        cfg.indexing.concurrency = 0;
        assert!(cfg.validate().is_err());
        cfg.indexing.concurrency = 8;

        cfg.indexing.max_file_size = 0;
        assert!(cfg.validate().is_err());
        cfg.indexing.max_file_size = 1000;

        cfg.search.default_limit = 0;
        assert!(cfg.validate().is_err());
        cfg.search.default_limit = 10;

        cfg.search.default_threshold = -0.5;
        assert!(cfg.validate().is_err());
        cfg.search.default_threshold = 1.5;
        assert!(cfg.validate().is_err());
        cfg.search.default_threshold = 0.3;

        cfg.provider.name = "invalid".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn config_validation_rejects_missing_gemini_key() {
        let mut cfg = Config::default();
        cfg.provider.name = "gemini".to_string();

        // No gemini config section
        cfg.provider.gemini = None;
        assert!(cfg.validate().is_err());

        // Empty key
        cfg.provider.gemini = Some(GeminiConfig {
            api_key: "".to_string(),
            model: "gemini-embedding-001".to_string(),
            dimensions: 768,
        });
        assert!(cfg.validate().is_err());

        // Placeholder key
        cfg.provider.gemini = Some(GeminiConfig {
            api_key: "your-api-key".to_string(),
            model: "gemini-embedding-001".to_string(),
            dimensions: 768,
        });
        assert!(cfg.validate().is_err());

        // Valid key
        cfg.provider.gemini = Some(GeminiConfig {
            api_key: "real-key".to_string(),
            model: "gemini-embedding-001".to_string(),
            dimensions: 768,
        });
        assert!(cfg.validate().is_ok());
    }
}
