//! ONNX Runtime embedding provider.
//!
//! Uses the `ort` crate for ONNX Runtime and `tokenizers` for HuggingFace tokenization.
//! Default model: all-MiniLM-L6-v2 (384 dimensions, 512 max tokens).

use crate::embedder::model_manager::ModelManager;
use crate::embedder::{Embedder, EmbedderResult};
use crate::error::VectorCodeError;
use async_trait::async_trait;
use ort::session::Session;
use ort::value::Tensor;
use std::path::PathBuf;
use std::time::Duration;
use tokenizers::Tokenizer;

/// Timeout for ONNX session creation (macOS CoreML EP can hang).
const SESSION_CREATION_TIMEOUT: Duration = Duration::from_secs(60);

/// ONNX Runtime embedding provider.
///
/// Loads a quantized all-MiniLM-L6-v2 model and produces 384-dimensional vectors.
/// The model and tokenizer are loaded from memory (bundled via include_bytes!).
pub struct OnnxEmbedder {
    session: tokio::sync::Mutex<Session>,
    tokenizer: Tokenizer,
}

impl OnnxEmbedder {
    /// Model name constant.
    pub const MODEL_NAME: &'static str = "all-MiniLM-L6-v2";

    /// Output dimensions.
    pub const DIMENSIONS: u32 = 384;

    /// Maximum token length.
    pub const MAX_TOKENS: u32 = 512;

    /// Create a new OnnxEmbedder from model and tokenizer bytes.
    ///
    /// # Arguments
    /// * `model_bytes` - ONNX model file contents
    /// * `tokenizer_bytes` - HuggingFace tokenizer.json contents
    ///
    /// # Errors
    /// Returns an error if the model or tokenizer fails to load.
    pub fn new(model_bytes: &[u8], tokenizer_bytes: &[u8]) -> EmbedderResult<Self> {
        // Load tokenizer
        let tokenizer =
            Tokenizer::from_bytes(tokenizer_bytes).map_err(|e| VectorCodeError::EmbedderError {
                message: format!("Failed to load tokenizer: {e}"),
            })?;

        // Load ONNX session from memory
        let mut builder = Session::builder().map_err(|e| VectorCodeError::EmbedderError {
            message: format!("Failed to create ONNX session builder: {e}"),
        })?;

        // If ORT_DISABLE_COREML is set, use CPU execution provider only.
        // This avoids the macOS CoreML EP hang during session creation.
        if std::env::var("ORT_DISABLE_COREML").is_ok() {
            builder = builder
                .with_execution_providers([ort::execution_providers::CPU::default().build()])
                .map_err(|e| VectorCodeError::EmbedderError {
                    message: format!("Failed to set CPU execution provider: {e}"),
                })?;
        }

        let session = builder.commit_from_memory(model_bytes).map_err(|e| {
            VectorCodeError::EmbedderError {
                message: format!("Failed to load ONNX model: {e}"),
            }
        })?;

        Ok(Self {
            session: tokio::sync::Mutex::new(session),
            tokenizer,
        })
    }

    /// Create an `OnnxEmbedder` from the cached model in `~/.vectorcode/models/`.
    ///
    /// Uses the default `ModelManager` to locate cached model files.
    /// Returns an actionable error if the model has not been downloaded.
    pub fn from_cache() -> EmbedderResult<Self> {
        let manager = ModelManager::new();
        Self::from_model_manager(&manager)
    }

    /// Create an `OnnxEmbedder` from a model cache at a specific directory.
    ///
    /// Primarily used for testing with temporary directories.
    pub fn from_model_dir(model_dir: PathBuf) -> EmbedderResult<Self> {
        let manager = ModelManager::with_model_dir(model_dir);
        Self::from_model_manager(&manager)
    }

    /// Internal: load model bytes from a `ModelManager` and construct the embedder.
    fn from_model_manager(manager: &ModelManager) -> EmbedderResult<Self> {
        let (model_bytes, tokenizer_bytes) = manager.load_model()?;
        Self::new(&model_bytes, &tokenizer_bytes)
    }

    /// Async wrapper around `from_cache()` with a 60-second timeout.
    ///
    /// Runs ONNX session creation on a raw OS thread (not tokio's blocking
    /// pool) so the process can exit even if the library hangs.
    pub async fn from_cache_with_timeout() -> EmbedderResult<Self> {
        let manager = ModelManager::new();
        Self::from_model_manager_with_timeout(&manager).await
    }

    /// Async wrapper around `from_model_dir()` with a 60-second timeout.
    ///
    /// Uses a raw OS thread so the process can exit even if the library hangs.
    pub async fn from_model_dir_with_timeout(model_dir: PathBuf) -> EmbedderResult<Self> {
        let manager = ModelManager::with_model_dir(model_dir);
        Self::from_model_manager_with_timeout(&manager).await
    }

    /// Internal: load from `ModelManager` with a timeout.
    ///
    /// Uses `std::thread::spawn` for the ONNX work so the thread is NOT
    /// tracked by tokio. This prevents tokio runtime shutdown from hanging
    /// when ONNX initialization blocks indefinitely (e.g. missing dylib,
    /// CoreML EP compile, or auto-download).
    ///
    /// The async side uses `tokio::sync::oneshot` — when the timeout fires,
    /// the receiver is dropped and the raw thread's `send()` fails gracefully.
    async fn from_model_manager_with_timeout(manager: &ModelManager) -> EmbedderResult<Self> {
        let (model_bytes, tokenizer_bytes) = manager.load_model()?;

        let (tx, rx) = tokio::sync::oneshot::channel::<EmbedderResult<Self>>();

        // Run ONNX session creation on a raw OS thread, NOT on tokio's
        // blocking pool.  If this thread hangs (missing dylib, CoreML EP
        // compile, etc.), the process can still exit because the thread is
        // not joined by the tokio runtime.
        std::thread::spawn(move || {
            let result = Self::new(&model_bytes, &tokenizer_bytes);
            // If the receiver was dropped (timeout), send fails — that's
            // fine, the thread just exits silently.
            let _ = tx.send(result);
        });

        match tokio::time::timeout(SESSION_CREATION_TIMEOUT, rx).await {
            Ok(Ok(embedder)) => embedder,
            Ok(Err(_recv_err)) => Err(VectorCodeError::EmbedderError {
                message: "ONNX session creation channel closed unexpectedly".to_string(),
            }),
            Err(_elapsed) => Err(VectorCodeError::EmbedderError {
                message: onnx_timeout_error_message(),
            }),
        }
    }

    /// Tokenize input text and prepare model inputs.
    ///
    /// Returns (input_ids, attention_mask, token_type_ids).
    /// Truncates to MAX_TOKENS if necessary.
    fn tokenize(&self, text: &str) -> EmbedderResult<(Vec<i64>, Vec<i64>, Vec<i64>)> {
        let encoding =
            self.tokenizer
                .encode(text, true)
                .map_err(|e| VectorCodeError::EmbedderError {
                    message: format!("Tokenization failed: {e}"),
                })?;

        let max_len = Self::MAX_TOKENS as usize;
        let ids: Vec<i64> = encoding
            .get_ids()
            .iter()
            .take(max_len)
            .map(|&x| x as i64)
            .collect();
        let mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .take(max_len)
            .map(|&x| x as i64)
            .collect();
        let type_ids: Vec<i64> = encoding
            .get_type_ids()
            .iter()
            .take(max_len)
            .map(|&x| x as i64)
            .collect();

        Ok((ids, mask, type_ids))
    }

    /// Apply mean pooling over token dimension and L2 normalize.
    fn pooling_and_normalize(
        last_hidden_state: &[f32],
        attention_mask: &[i64],
        seq_len: usize,
        dims: usize,
    ) -> Vec<f32> {
        // last_hidden_state shape: [seq_len * dims] (batch dim already stripped)
        let mut pooled = vec![0.0f32; dims];
        let mut sum_mask = 0.0f32;

        for i in 0..seq_len {
            let mask_val = attention_mask[i] as f32;
            sum_mask += mask_val;
            for j in 0..dims {
                pooled[j] += last_hidden_state[i * dims + j] * mask_val;
            }
        }

        // Mean pooling
        if sum_mask > 0.0 {
            for val in pooled.iter_mut() {
                *val /= sum_mask;
            }
        }

        // L2 normalize
        let norm: f32 = pooled.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for val in pooled.iter_mut() {
                *val /= norm;
            }
        }

        pooled
    }
}

/// Build the error message for ONNX session creation timeout.
pub fn onnx_timeout_error_message() -> String {
    "ONNX Runtime initialization timed out after 60s.\n\
     \n\
     This usually means the ONNX Runtime shared library (libonnxruntime.dylib)\n\
     is not installed or is hanging during initialization.\n\
     \n\
     Fix options:\n\
       • Install ONNX Runtime:  brew install onnxruntime\n\
       • Or switch to a different provider:  vectorcode init --provider ollama\n\
       • Or set ORT_DISABLE_COREML=1 if CoreML EP is the culprit"
        .to_string()
}

#[async_trait]
impl Embedder for OnnxEmbedder {
    async fn embed(&self, text: &str) -> EmbedderResult<Vec<f32>> {
        let (input_ids, attention_mask, token_type_ids) = self.tokenize(text)?;
        let seq_len = input_ids.len();
        let dims = Self::DIMENSIONS as usize;

        // Prepare input tensors: shape (1, seq_len)
        let input_ids_tensor =
            Tensor::from_array(([1usize, seq_len], input_ids.into_boxed_slice())).map_err(|e| {
                VectorCodeError::EmbedderError {
                    message: format!("Failed to create input_ids tensor: {e}"),
                }
            })?;

        let attention_mask_tensor =
            Tensor::from_array(([1usize, seq_len], attention_mask.clone().into_boxed_slice()))
                .map_err(|e| VectorCodeError::EmbedderError {
                    message: format!("Failed to create attention_mask tensor: {e}"),
                })?;

        let token_type_ids_tensor =
            Tensor::from_array(([1usize, seq_len], token_type_ids.into_boxed_slice())).map_err(
                |e| VectorCodeError::EmbedderError {
                    message: format!("Failed to create token_type_ids tensor: {e}"),
                },
            )?;

        // Run session with named inputs — scoped so lock is released promptly
        let output_vec: Vec<f32> = {
            let mut session = self.session.lock().await;
            let outputs = session
                .run(ort::inputs![
                    "input_ids" => input_ids_tensor,
                    "attention_mask" => attention_mask_tensor,
                    "token_type_ids" => token_type_ids_tensor,
                ])
                .map_err(|e| VectorCodeError::EmbedderError {
                    message: format!("ONNX session run failed: {e}"),
                })?;

            // Extract last_hidden_state — use .get() to avoid panic on unexpected output names
            let (_, output_data) = outputs
                .get("last_hidden_state")
                .ok_or_else(|| VectorCodeError::EmbedderError {
                    message: format!(
                        "ONNX model output 'last_hidden_state' not found. \
                         Available outputs: {:?}. \
                         This embedder requires a MiniLM-L6-v2 model. \
                         Try 'vectorcode init --provider onnx' to re-download.",
                        outputs.keys().collect::<Vec<_>>()
                    ),
                })?
                .try_extract_tensor::<f32>()
                .map_err(|e| VectorCodeError::EmbedderError {
                    message: format!("Failed to extract output tensor: {e}"),
                })?;
            output_data.to_vec()
        }; // Lock released here

        // Apply mean pooling and normalize
        Ok(Self::pooling_and_normalize(
            &output_vec,
            &attention_mask,
            seq_len,
            dims,
        ))
    }

    fn dimensions(&self) -> u32 {
        Self::DIMENSIONS
    }

    fn provider_name(&self) -> &str {
        "onnx"
    }

    fn model_name(&self) -> &str {
        Self::MODEL_NAME
    }

    fn max_tokens(&self) -> u32 {
        Self::MAX_TOKENS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onnx_embedder_metadata_constants() {
        assert_eq!(OnnxEmbedder::DIMENSIONS, 384);
        assert_eq!(OnnxEmbedder::MAX_TOKENS, 512);
        assert_eq!(OnnxEmbedder::MODEL_NAME, "all-MiniLM-L6-v2");
    }

    #[test]
    fn onnx_embedder_new_fails_with_invalid_model() {
        let invalid_model = b"not a valid onnx model";
        let invalid_tokenizer = b"not a valid tokenizer";
        let result = OnnxEmbedder::new(invalid_model, invalid_tokenizer);
        assert!(result.is_err(), "Should fail with invalid model bytes");
    }

    #[test]
    fn onnx_embedder_new_fails_with_invalid_tokenizer() {
        // Even with valid-looking model bytes, invalid tokenizer should fail
        let invalid_tokenizer = b"{}"; // Valid JSON but not a tokenizer
        let result = OnnxEmbedder::new(b"model", invalid_tokenizer);
        assert!(result.is_err(), "Should fail with invalid tokenizer");
    }

    #[test]
    fn from_cache_errors_when_model_not_downloaded() {
        let empty_dir = tempfile::tempdir().unwrap();
        let result = OnnxEmbedder::from_model_dir(empty_dir.path().to_path_buf());
        assert!(
            result.is_err(),
            "from_cache should fail when model files are missing"
        );
        let err_msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => unreachable!(),
        };
        assert!(
            err_msg.contains("vectorcode init"),
            "Error should suggest running init, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn from_cache_with_timeout_errors_when_model_not_downloaded() {
        let empty_dir = tempfile::tempdir().unwrap();
        let manager = crate::embedder::model_manager::ModelManager::with_model_dir(
            empty_dir.path().to_path_buf(),
        );
        let result = OnnxEmbedder::from_model_manager_with_timeout(&manager).await;
        assert!(
            result.is_err(),
            "from_cache_with_timeout should fail when model files are missing"
        );
        let err_msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => unreachable!(),
        };
        assert!(
            err_msg.contains("vectorcode init"),
            "Error should suggest running init, got: {err_msg}"
        );
    }

    #[tokio::test]
    async fn from_cache_with_timeout_returns_error_not_panic() {
        let empty_dir = tempfile::tempdir().unwrap();
        let result =
            OnnxEmbedder::from_model_dir_with_timeout(empty_dir.path().to_path_buf()).await;
        assert!(
            result.is_err(),
            "Should return error, not panic, when model is missing"
        );
    }

    #[test]
    fn timeout_error_message_contains_helpful_hints() {
        let msg = crate::embedder::onnx::onnx_timeout_error_message();
        assert!(
            msg.contains("timed out"),
            "Error should mention 'timed out', got: {msg}"
        );
        assert!(
            msg.contains("ORT_DISABLE_COREML"),
            "Error should mention 'ORT_DISABLE_COREML', got: {msg}"
        );
    }

    #[test]
    fn from_cache_errors_with_invalid_model_bytes_in_cache() {
        let dir = tempfile::tempdir().unwrap();
        // Write fake model + invalid tokenizer to simulate corrupted cache
        std::fs::write(dir.path().join("model.onnx"), b"fake-model").unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), b"not-json").unwrap();
        let result = OnnxEmbedder::from_model_dir(dir.path().to_path_buf());
        assert!(
            result.is_err(),
            "from_cache should fail with corrupted model files"
        );
    }

    #[test]
    fn from_model_dir_returns_error_when_only_model_exists() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("model.onnx"), b"model").unwrap();
        // tokenizer.json missing
        let result = OnnxEmbedder::from_model_dir(dir.path().to_path_buf());
        assert!(
            result.is_err(),
            "Should fail when tokenizer.json is missing"
        );
        let err_msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => unreachable!(),
        };
        assert!(
            err_msg.contains("vectorcode init"),
            "Error should suggest running init, got: {err_msg}"
        );
    }

    #[test]
    fn pooling_and_normalize_produces_unit_vector() {
        // Simulate a 2-token, 4-dim output
        let hidden_state = vec![
            1.0, 0.0, 0.0, 0.0, // token 0
            0.0, 1.0, 0.0, 0.0, // token 1
        ];
        let attention_mask = vec![1i64, 1];
        let result = OnnxEmbedder::pooling_and_normalize(&hidden_state, &attention_mask, 2, 4);
        assert_eq!(result.len(), 4);
        // Mean of [1,0,0,0] and [0,1,0,0] = [0.5, 0.5, 0, 0]
        // L2 norm = sqrt(0.25 + 0.25) = sqrt(0.5) ≈ 0.7071
        // Normalized: [0.5/0.7071, 0.5/0.7071, 0, 0] ≈ [0.7071, 0.7071, 0, 0]
        let norm: f32 = result.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "Result should be L2-normalized, got norm={norm}"
        );
        assert!(
            (result[0] - result[1]).abs() < 1e-5,
            "First two dims should be equal after mean pooling"
        );
    }

    #[test]
    fn pooling_ignores_padded_tokens() {
        // 3 tokens but only first 2 are real (third is padding)
        let hidden_state = vec![
            1.0, 0.0, // token 0
            0.0, 1.0, // token 1
            9.0, 9.0, // token 2 (padding — should be ignored)
        ];
        let attention_mask = vec![1i64, 1, 0]; // third token masked out
        let result = OnnxEmbedder::pooling_and_normalize(&hidden_state, &attention_mask, 3, 2);
        // Mean of [1,0] and [0,1] only = [0.5, 0.5], then normalized
        assert!(
            (result[0] - result[1]).abs() < 1e-5,
            "Padding should not affect result"
        );
        let norm: f32 = result.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "Result should be L2-normalized");
    }
}
