/// Central error type for all VectorCode operations.
///
/// Each variant maps to a distinct failure mode described in spec §18.1.
/// `Database` and `Io` use `#[from]` for ergonomic `?` propagation.
#[derive(thiserror::Error, Debug)]
pub enum VectorCodeError {
    #[error("Index not initialized. Run `vectorcode init` first.")]
    NotInitialized,

    #[error(
        "Index was created with provider '{expected}' ({expected_dims}d) \
         but current config uses '{actual}' ({actual_dims}d). \
         Run `vectorcode index --full` to rebuild."
    )]
    ProviderMismatch {
        expected: String,
        expected_dims: u32,
        actual: String,
        actual_dims: u32,
    },

    #[error("Embedding provider error: {message}")]
    EmbedderError { message: String },

    #[error("API rate limited. Retrying in {retry_after_secs}s...")]
    RateLimited { retry_after_secs: u64 },

    #[error("Ollama not reachable at {url}. Is it running? Try: ollama serve")]
    OllamaUnavailable { url: String },

    #[error("Model '{model}' not found in Ollama. Try: ollama pull {model}")]
    OllamaModelNotFound { model: String },

    #[error("API key not set. Set {env_var} or configure in .vectorcode/config.toml")]
    ApiKeyMissing { env_var: String },

    #[error("Tree-sitter parse error for {file_path}: {message}")]
    ParseError { file_path: String, message: String },

    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("FTS5 error: {message}")]
    Fts5Error { message: String },

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Reranker error: {message}")]
    RerankerError { message: String },

    #[error("Graph query failed: {message}")]
    GraphQueryFailed { message: String },

    #[error("Path '{path}' is outside of all initialized workspaces")]
    PathOutsideAnyWorkspace { path: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_initialized_display_message() {
        let err = VectorCodeError::NotInitialized;
        let msg = err.to_string();
        assert!(
            msg.contains("not initialized"),
            "Expected 'not initialized' in message, got: {msg}"
        );
        assert!(
            msg.contains("vectorcode init"),
            "Expected 'vectorcode init' hint in message, got: {msg}"
        );
    }

    #[test]
    fn provider_mismatch_includes_both_providers_and_dims() {
        let err = VectorCodeError::ProviderMismatch {
            expected: "onnx".to_string(),
            expected_dims: 384,
            actual: "gemini".to_string(),
            actual_dims: 768,
        };
        let msg = err.to_string();
        assert!(msg.contains("onnx"), "Missing expected provider: {msg}");
        assert!(msg.contains("384"), "Missing expected dims: {msg}");
        assert!(msg.contains("gemini"), "Missing actual provider: {msg}");
        assert!(msg.contains("768"), "Missing actual dims: {msg}");
    }

    #[test]
    fn embedder_error_carries_message() {
        let err = VectorCodeError::EmbedderError {
            message: "model load failed".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("model load failed"),
            "Expected error detail in message, got: {msg}"
        );
    }

    #[test]
    fn rate_limited_shows_retry_after() {
        let err = VectorCodeError::RateLimited {
            retry_after_secs: 30,
        };
        let msg = err.to_string();
        assert!(msg.contains("30"), "Missing retry seconds: {msg}");
    }

    #[test]
    fn ollama_unavailable_shows_url() {
        let err = VectorCodeError::OllamaUnavailable {
            url: "http://localhost:11434".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("http://localhost:11434"), "Missing URL: {msg}");
        assert!(msg.contains("ollama serve"), "Missing recovery hint: {msg}");
    }

    #[test]
    fn ollama_model_not_found_shows_pull_command() {
        let err = VectorCodeError::OllamaModelNotFound {
            model: "embeddinggemma:latest".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("embeddinggemma:latest"),
            "Missing model name: {msg}"
        );
        assert!(msg.contains("ollama pull"), "Missing pull hint: {msg}");
    }

    #[test]
    fn api_key_missing_names_env_var() {
        let err = VectorCodeError::ApiKeyMissing {
            env_var: "GEMINI_API_KEY".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("GEMINI_API_KEY"),
            "Missing env var name: {msg}"
        );
    }

    #[test]
    fn parse_error_includes_file_and_message() {
        let err = VectorCodeError::ParseError {
            file_path: "src/main.ts".to_string(),
            message: "unexpected token".to_string(),
        };
        let msg = err.to_string();
        assert!(msg.contains("src/main.ts"), "Missing file path: {msg}");
        assert!(
            msg.contains("unexpected token"),
            "Missing parse detail: {msg}"
        );
    }

    #[test]
    fn database_converts_from_rusqlite_error() {
        // Trigger a real rusqlite error by opening a path in a nonexistent directory
        let rusqlite_err = rusqlite::Connection::open("/nonexistent/dir/db.sqlite").unwrap_err();
        let err: VectorCodeError = rusqlite_err.into();
        let msg = err.to_string();
        assert!(
            msg.contains("Database error"),
            "Expected 'Database error' prefix, got: {msg}"
        );
    }

    #[test]
    fn fts5_error_display_message() {
        let err = VectorCodeError::Fts5Error {
            message: "FTS5 query syntax error".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("FTS5 error"),
            "Expected 'FTS5 error' prefix, got: {msg}"
        );
        assert!(
            msg.contains("FTS5 query syntax error"),
            "Expected detail message, got: {msg}"
        );
    }

    #[test]
    fn io_converts_from_std_io_error() {
        let io_err = std::fs::read_to_string("/nonexistent/path/that/does/not/exist").unwrap_err();
        let err: VectorCodeError = io_err.into();
        let msg = err.to_string();
        assert!(
            msg.contains("IO error") || msg.contains("No such file"),
            "Expected IO error message, got: {msg}"
        );
    }

    #[test]
    fn reranker_error_display_message() {
        let err = VectorCodeError::RerankerError {
            message: "model load failed".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("Reranker error"),
            "Expected 'Reranker error' prefix, got: {msg}"
        );
        assert!(
            msg.contains("model load failed"),
            "Expected detail message, got: {msg}"
        );
    }

    #[test]
    fn graph_query_failed_display_message() {
        let err = VectorCodeError::GraphQueryFailed {
            message: "connection timeout".to_string(),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("Graph query failed"),
            "Expected 'Graph query failed' prefix, got: {msg}"
        );
        assert!(
            msg.contains("connection timeout"),
            "Expected detail message, got: {msg}"
        );
    }
}
