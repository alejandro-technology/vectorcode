//! ONNX model manager — download, cache, and load models from HuggingFace.
//!
//! Manages the lifecycle of ONNX model files: downloading from HuggingFace CDN,
//! caching in `~/.vectorcode/models/`, and loading into memory for `OnnxEmbedder`.

use std::path::PathBuf;

use crate::error::VectorCodeError;

/// Manages ONNX model download, caching, and loading.
///
/// Models are stored in `~/.vectorcode/models/minilm-l6-v2-q8/` by default.
/// The model directory can be overridden for testing via `ModelManager::with_model_dir()`.
pub struct ModelManager {
    model_dir: PathBuf,
}

/// HuggingFace CDN base URL for all-MiniLM-L6-v2 model files.
const HF_BASE_URL: &str =
    "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main";

/// Filename for the ONNX model within the model directory (always stored as model.onnx locally).
const MODEL_FILENAME: &str = "model.onnx";

/// HuggingFace path for the quantized ONNX model, selected by platform.
/// Falls back to the unquantized `onnx/model.onnx` (~90MB) if no quantized variant matches.
const ONNX_MODEL_PATH: &str = {
    if cfg!(all(target_arch = "aarch64", any(target_os = "macos", target_os = "linux"))) {
        "onnx/model_qint8_arm64.onnx" // ~23MB, optimized for ARM64
    } else if cfg!(all(
        target_arch = "x86_64",
        any(target_os = "linux", target_os = "windows")
    )) {
        "onnx/model_quint8_avx2.onnx" // ~23MB, optimized for x86_64 with AVX2
    } else {
        "onnx/model.onnx" // ~90MB, unquantized fallback
    }
};

/// Filename for the HuggingFace tokenizer within the model directory.
const TOKENIZER_FILENAME: &str = "tokenizer.json";

impl ModelManager {
    /// Create a `ModelManager` with the default model directory.
    ///
    /// Path: `~/.vectorcode/models/minilm-l6-v2-q8/`
    pub fn new() -> Self {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        let model_dir = PathBuf::from(home)
            .join(".vectorcode")
            .join("models")
            .join("minilm-l6-v2-q8");
        Self { model_dir }
    }

    /// Create a `ModelManager` with a custom model directory (for testing).
    pub fn with_model_dir(dir: PathBuf) -> Self {
        Self { model_dir: dir }
    }

    /// Return the path to the model cache directory.
    pub fn model_dir(&self) -> &std::path::Path {
        &self.model_dir
    }

    /// Check whether both `model.onnx` and `tokenizer.json` are present in the cache.
    pub fn is_downloaded(&self) -> bool {
        let model_path = self.model_dir.join(MODEL_FILENAME);
        let tokenizer_path = self.model_dir.join(TOKENIZER_FILENAME);
        model_path.exists() && tokenizer_path.exists()
    }

    /// Download the quantized ONNX model and tokenizer from HuggingFace.
    ///
    /// Downloads to temporary files first, then atomically renames to the final
    /// paths to avoid partial files on interruption.
    ///
    /// # Errors
    /// Returns an error if the HTTP request fails or the files cannot be written.
    pub async fn download_model(&self) -> Result<(), VectorCodeError> {
        let model_url = format!("{HF_BASE_URL}/{ONNX_MODEL_PATH}");
        let tokenizer_url = format!("{HF_BASE_URL}/tokenizer.json");

        self.download_from(&model_url, &tokenizer_url).await
    }

    /// Download model and tokenizer from the given URLs.
    ///
    /// This is the internal implementation used by `download_model()`, exposed
    /// for testability with custom URLs.
    async fn download_from(
        &self,
        model_url: &str,
        tokenizer_url: &str,
    ) -> Result<(), VectorCodeError> {
        std::fs::create_dir_all(&self.model_dir)?;

        let client = reqwest::Client::new();

        // Download model file
        let model_bytes = Self::fetch_with_progress(&client, model_url, MODEL_FILENAME).await?;
        let tmp_model = self.model_dir.join("model.onnx.tmp");
        std::fs::write(&tmp_model, &model_bytes)?;
        let final_model = self.model_dir.join(MODEL_FILENAME);
        std::fs::rename(&tmp_model, &final_model)?;

        // Download tokenizer file
        let tokenizer_bytes =
            Self::fetch_with_progress(&client, tokenizer_url, TOKENIZER_FILENAME).await?;
        let tmp_tokenizer = self.model_dir.join("tokenizer.json.tmp");
        std::fs::write(&tmp_tokenizer, &tokenizer_bytes)?;
        let final_tokenizer = self.model_dir.join(TOKENIZER_FILENAME);
        std::fs::rename(&tmp_tokenizer, &final_tokenizer)?;

        tracing::info!(
            model_size = model_bytes.len(),
            tokenizer_size = tokenizer_bytes.len(),
            "Model download complete"
        );

        Ok(())
    }

    /// Fetch a URL with an indicatif progress bar, returning the bytes.
    async fn fetch_with_progress(
        client: &reqwest::Client,
        url: &str,
        label: &str,
    ) -> Result<Vec<u8>, VectorCodeError> {
        let mut response =
            client
                .get(url)
                .send()
                .await
                .map_err(|e| VectorCodeError::EmbedderError {
                    message: format!("Failed to download {label}: {e}"),
                })?;

        if !response.status().is_success() {
            return Err(VectorCodeError::EmbedderError {
                message: format!(
                    "Failed to download {label}: HTTP {}",
                    response.status().as_u16()
                ),
            });
        }

        let total_size = response.content_length().unwrap_or(0);
        let pb = if total_size > 0 {
            let pb = indicatif::ProgressBar::new(total_size);
            pb.set_style(
                indicatif::ProgressStyle::default_bar()
                    .template(&format!(
                        "{{spinner:.green}} {label} [{{bar:40.cyan/blue}}] {{bytes}}/{{total_bytes}} ({{eta}})"
                    ))
                    .unwrap_or_else(|_| indicatif::ProgressStyle::default_bar())
                    .progress_chars("=>-"),
            );
            pb
        } else {
            let pb = indicatif::ProgressBar::new_spinner();
            pb.set_message(format!("Downloading {label}..."));
            pb
        };

        let mut bytes = Vec::with_capacity(total_size as usize);
        while let Some(chunk) =
            response
                .chunk()
                .await
                .map_err(|e| VectorCodeError::EmbedderError {
                    message: format!("Failed to read {label} chunk: {e}"),
                })?
        {
            bytes.extend_from_slice(&chunk);
            pb.set_position(bytes.len() as u64);
        }
        pb.finish_and_clear();

        Ok(bytes)
    }

    /// Load the cached model and tokenizer bytes into memory.
    ///
    /// Returns `(model_bytes, tokenizer_bytes)`.
    ///
    /// # Errors
    /// Returns an error if the files are not cached. Suggests running `vectorcode init` first.
    pub fn load_model(&self) -> Result<(Vec<u8>, Vec<u8>), VectorCodeError> {
        if !self.is_downloaded() {
            return Err(VectorCodeError::EmbedderError {
                message: "ONNX model not found in cache. Run `vectorcode init --provider onnx` to download it.".to_string(),
            });
        }

        let model_bytes = std::fs::read(self.model_dir.join(MODEL_FILENAME))?;
        let tokenizer_bytes = std::fs::read(self.model_dir.join(TOKENIZER_FILENAME))?;

        Ok((model_bytes, tokenizer_bytes))
    }
}

impl Default for ModelManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::TcpListener;

    // ── model_dir() ──────────────────────────────────────────────────

    #[test]
    fn model_dir_ends_with_expected_folder() {
        let manager = ModelManager::new();
        let dir = manager.model_dir();
        assert!(
            dir.to_str().unwrap().ends_with("minilm-l6-v2-q8"),
            "Expected path ending with 'minilm-l6-v2-q8', got: {}",
            dir.display()
        );
    }

    #[test]
    fn model_dir_contains_vectorcode_models() {
        let manager = ModelManager::new();
        let dir = manager.model_dir();
        let path_str = dir.to_str().unwrap();
        assert!(
            path_str.contains(".vectorcode/models"),
            "Expected path containing '.vectorcode/models', got: {path_str}"
        );
    }

    #[test]
    fn model_dir_uses_custom_path() {
        let custom = PathBuf::from("/tmp/test-models");
        let manager = ModelManager::with_model_dir(custom.clone());
        assert_eq!(manager.model_dir(), custom.as_path());
    }

    // ── is_downloaded() ──────────────────────────────────────────────

    #[test]
    fn is_downloaded_false_when_dir_empty() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ModelManager::with_model_dir(dir.path().to_path_buf());
        assert!(!manager.is_downloaded());
    }

    #[test]
    fn is_downloaded_true_when_both_files_exist() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.onnx"), b"model").unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), b"tokenizer").unwrap();
        let manager = ModelManager::with_model_dir(dir.path().to_path_buf());
        assert!(manager.is_downloaded());
    }

    #[test]
    fn is_downloaded_false_when_only_model_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.onnx"), b"model").unwrap();
        let manager = ModelManager::with_model_dir(dir.path().to_path_buf());
        assert!(!manager.is_downloaded());
    }

    #[test]
    fn is_downloaded_false_when_only_tokenizer_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), b"tokenizer").unwrap();
        let manager = ModelManager::with_model_dir(dir.path().to_path_buf());
        assert!(!manager.is_downloaded());
    }

    #[test]
    fn is_downloaded_false_when_dir_does_not_exist() {
        let manager =
            ModelManager::with_model_dir(PathBuf::from("/nonexistent/path/that/does/not/exist"));
        assert!(!manager.is_downloaded());
    }

    // ── load_model() ─────────────────────────────────────────────────

    #[test]
    fn load_model_errors_when_not_downloaded() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ModelManager::with_model_dir(dir.path().to_path_buf());
        let result = manager.load_model();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("vectorcode init"),
            "Error should suggest running init, got: {err_msg}"
        );
    }

    #[test]
    fn load_model_returns_correct_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let model_data = b"fake-onnx-model-bytes-12345";
        let tokenizer_data = br#"{"version":"1.0","model":"test"}"#;
        std::fs::write(dir.path().join("model.onnx"), model_data).unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), tokenizer_data).unwrap();

        let manager = ModelManager::with_model_dir(dir.path().to_path_buf());
        let (model, tokenizer) = manager.load_model().unwrap();
        assert_eq!(model, model_data);
        assert_eq!(tokenizer, tokenizer_data);
    }

    #[test]
    fn load_model_returns_large_bytes_intact() {
        let dir = tempfile::tempdir().unwrap();
        // 10KB of deterministic data
        let model_data: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
        let tokenizer_data: Vec<u8> = (0..5_000).map(|i| (255 - i % 256) as u8).collect();
        std::fs::write(dir.path().join("model.onnx"), &model_data).unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), &tokenizer_data).unwrap();

        let manager = ModelManager::with_model_dir(dir.path().to_path_buf());
        let (model, tokenizer) = manager.load_model().unwrap();
        assert_eq!(model.len(), 10_000);
        assert_eq!(tokenizer.len(), 5_000);
        assert_eq!(model, model_data.as_slice());
        assert_eq!(tokenizer, tokenizer_data.as_slice());
    }

    // ── download_from() (mock HTTP) ──────────────────────────────────

    /// Spawn a minimal HTTP server on localhost that serves `body` at any path.
    /// Returns the base URL (e.g. `http://127.0.0.1:PORT`).
    async fn spawn_test_server(body: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server_body = body.clone();

        tokio::spawn(async move {
            // Serve up to 2 requests (model + tokenizer)
            for _ in 0..2 {
                if let Ok((mut socket, _)) = listener.accept().await {
                    let mut buf = [0u8; 4096];
                    let _ = tokio::io::AsyncReadExt::read(&mut socket, &mut buf).await;

                    let response_body = &server_body;
                    let header = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\n\r\n",
                        response_body.len()
                    );
                    let _ =
                        tokio::io::AsyncWriteExt::write_all(&mut socket, header.as_bytes()).await;
                    let _ = tokio::io::AsyncWriteExt::write_all(&mut socket, response_body).await;
                }
            }
        });

        format!("http://127.0.0.1:{}", addr.port())
    }

    #[tokio::test]
    async fn download_from_creates_files_with_correct_content() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ModelManager::with_model_dir(dir.path().to_path_buf());

        let model_content = b"test-model-binary-content";
        let server_url = spawn_test_server(model_content.to_vec()).await;

        let model_url = format!("{}/model.onnx", server_url);
        let tokenizer_url = format!("{}/tokenizer.json", server_url);

        manager
            .download_from(&model_url, &tokenizer_url)
            .await
            .unwrap();

        let saved_model = std::fs::read(dir.path().join("model.onnx")).unwrap();
        let saved_tokenizer = std::fs::read(dir.path().join("tokenizer.json")).unwrap();
        assert_eq!(saved_model, model_content);
        assert_eq!(saved_tokenizer, model_content);
    }

    #[tokio::test]
    async fn download_from_creates_directory_if_missing() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("deep").join("nested").join("models");
        let manager = ModelManager::with_model_dir(nested.clone());

        let content = b"model-data";
        let server_url = spawn_test_server(content.to_vec()).await;

        manager
            .download_from(
                &format!("{}/model.onnx", server_url),
                &format!("{}/tokenizer.json", server_url),
            )
            .await
            .unwrap();

        assert!(nested.join("model.onnx").exists());
        assert!(nested.join("tokenizer.json").exists());
    }

    #[tokio::test]
    async fn download_from_makes_is_downloaded_true() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ModelManager::with_model_dir(dir.path().to_path_buf());
        assert!(!manager.is_downloaded());

        let content = b"model-bytes";
        let server_url = spawn_test_server(content.to_vec()).await;

        manager
            .download_from(
                &format!("{}/model.onnx", server_url),
                &format!("{}/tokenizer.json", server_url),
            )
            .await
            .unwrap();

        assert!(manager.is_downloaded());
    }

    #[tokio::test]
    async fn download_from_bad_url_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let manager = ModelManager::with_model_dir(dir.path().to_path_buf());

        // Port 1 is typically unused → connection refused
        let result = manager
            .download_from(
                "http://127.0.0.1:1/model.onnx",
                "http://127.0.0.1:1/tokenizer.json",
            )
            .await;

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("download") || err_msg.contains("Failed"),
            "Expected download error, got: {err_msg}"
        );
    }
}
