//! ONNX Runtime reranker provider.
//!
//! Uses the BGE-Reranker-v2-m3 cross-encoder model (XLM-R based, ~571 MB int8 ONNX)
//! to re-score candidate documents against a query. The model produces a single logit
//! per query-document pair; sigmoid maps it to a relevance score in [0, 1].
//!
//! Follows the same patterns as `OnnxEmbedder`: `tokio::sync::Mutex<Session>`,
//! raw-thread-spawn load with 60 s timeout, and `ModelManager` cache reuse.

use crate::embedder::model_manager::ModelManager;
use crate::error::VectorCodeError;
use crate::reranker::{RerankDocument, Reranker, Result as RerankResult};
use async_trait::async_trait;
use ort::session::Session;
use ort::value::Tensor;
use std::path::PathBuf;
use std::time::Duration;
use tokenizers::Tokenizer;

/// Timeout for ONNX session creation (macOS CoreML EP can hang).
const SESSION_CREATION_TIMEOUT: Duration = Duration::from_secs(60);

/// HuggingFace CDN base URL for BGE-Reranker-v2-m3 (onnx-community ONNX export).
const HF_BASE_URL: &str =
    "https://huggingface.co/onnx-community/bge-reranker-v2-m3-ONNX/resolve/main";

/// ONNX model path within the HuggingFace repo.
///
/// Uses the self-contained quantized variant. The unquantized `model.onnx`
/// relies on external data (`model.onnx_data`) which `commit_from_memory`
/// cannot resolve — it must be loaded from disk files.
const ONNX_MODEL_PATH: &str = "onnx/model_quantized.onnx";

/// Filename for the tokenizer within the model directory.
const TOKENIZER_FILENAME: &str = "tokenizer.json";

/// Filename for the ONNX model within the model directory (stored locally as model.onnx).
const MODEL_FILENAME: &str = "model.onnx";

/// ONNX cross-encoder reranker using BGE-Reranker-v2-m3.
///
/// Loads a quantized cross-encoder model that scores query-document pairs.
/// The model and tokenizer are loaded from the `~/.vectorcode/models/` cache.
pub struct OnnxReranker {
    session: tokio::sync::Mutex<Session>,
    tokenizer: Tokenizer,
}

impl OnnxReranker {
    /// Model name constant.
    pub const MODEL_NAME: &'static str = "BGE-Reranker-v2-m3";

    /// Maximum token length for cross-encoder input (XLM-R context window).
    pub const MAX_TOKENS: u32 = 8192;

    /// Create a new `OnnxReranker` from model and tokenizer bytes.
    ///
    /// # Arguments
    /// * `model_bytes` - ONNX model file contents
    /// * `tokenizer_bytes` - HuggingFace tokenizer.json contents
    ///
    /// # Errors
    /// Returns a `RerankerError` if the model or tokenizer fails to load.
    pub fn new(model_bytes: &[u8], tokenizer_bytes: &[u8]) -> RerankResult<Self> {
        let tokenizer =
            Tokenizer::from_bytes(tokenizer_bytes).map_err(|e| VectorCodeError::RerankerError {
                message: format!("Failed to load reranker tokenizer: {e}"),
            })?;

        let mut builder = Session::builder().map_err(|e| VectorCodeError::RerankerError {
            message: format!("Failed to create ONNX session builder: {e}"),
        })?;

        // If ORT_DISABLE_COREML is set, use CPU execution provider only.
        if std::env::var("ORT_DISABLE_COREML").is_ok() {
            builder = builder
                .with_execution_providers([ort::execution_providers::CPU::default().build()])
                .map_err(|e| VectorCodeError::RerankerError {
                    message: format!("Failed to set CPU execution provider: {e}"),
                })?;
        }

        let session = builder.commit_from_memory(model_bytes).map_err(|e| {
            VectorCodeError::RerankerError {
                message: format!("Failed to load reranker ONNX model: {e}"),
            }
        })?;

        Ok(Self {
            session: tokio::sync::Mutex::new(session),
            tokenizer,
        })
    }

    /// Create an `OnnxReranker` from the cached model in `~/.vectorcode/models/`.
    ///
    /// Returns an actionable error if the model has not been downloaded.
    pub fn from_cache() -> RerankResult<Self> {
        let manager = RerankerModelManager::new();
        Self::from_reranker_model_manager(&manager)
    }

    /// Create an `OnnxReranker` from a model cache at a specific directory.
    ///
    /// Primarily used for testing with temporary directories.
    pub fn from_model_dir(model_dir: PathBuf) -> RerankResult<Self> {
        let manager = RerankerModelManager::with_model_dir(model_dir);
        Self::from_reranker_model_manager(&manager)
    }

    /// Internal: load model bytes from a `RerankerModelManager` and construct the reranker.
    fn from_reranker_model_manager(manager: &RerankerModelManager) -> RerankResult<Self> {
        let (model_bytes, tokenizer_bytes) = manager.load_model()?;
        Self::new(&model_bytes, &tokenizer_bytes)
    }

    /// Async wrapper around `from_cache()` with a 60-second timeout.
    ///
    /// If the model is not cached locally, it is downloaded automatically
    /// from HuggingFace (~571 MB).  After download, ONNX session creation
    /// runs on a raw OS thread (not tokio's blocking pool) so the process
    /// can exit even if the library hangs.
    pub async fn from_cache_with_timeout() -> RerankResult<Self> {
        let manager = RerankerModelManager::new();

        // Download the model on first use if it is not already cached.
        if !manager.is_downloaded() {
            tracing::info!(
                "Reranker model not found in cache. Downloading BGE-Reranker-v2-m3 (~571 MB) ..."
            );
            manager.download_model().await.map_err(|e| VectorCodeError::RerankerError {
                message: format!(
                    "Failed to download reranker model from HuggingFace: {e}\n\
                     You can also download it manually from https://huggingface.co/Xenova/bge-reranker-v2-m3\n\
                     and place model.onnx + tokenizer.json into ~/.vectorcode/models/bge-reranker-v2-m3/"
                ),
            })?;
        }

        Self::from_reranker_model_manager_with_timeout(&manager).await
    }

    /// Async wrapper around `from_model_dir()` with a 60-second timeout.
    ///
    /// Uses a raw OS thread so the process can exit even if the library hangs.
    pub async fn from_model_dir_with_timeout(model_dir: PathBuf) -> RerankResult<Self> {
        let manager = RerankerModelManager::with_model_dir(model_dir);
        Self::from_reranker_model_manager_with_timeout(&manager).await
    }

    /// Internal: load from `RerankerModelManager` with a timeout.
    ///
    /// Uses `std::thread::spawn` for the ONNX work so the thread is NOT
    /// tracked by tokio. This prevents tokio runtime shutdown from hanging
    /// when ONNX initialization blocks indefinitely.
    async fn from_reranker_model_manager_with_timeout(
        manager: &RerankerModelManager,
    ) -> RerankResult<Self> {
        let (model_bytes, tokenizer_bytes) = manager.load_model()?;

        let (tx, rx) = tokio::sync::oneshot::channel::<RerankResult<Self>>();

        // Run ONNX session creation on a raw OS thread, NOT on tokio's
        // blocking pool. If this thread hangs, the process can still exit.
        std::thread::spawn(move || {
            let result = Self::new(&model_bytes, &tokenizer_bytes);
            let _ = tx.send(result);
        });

        match tokio::time::timeout(SESSION_CREATION_TIMEOUT, rx).await {
            Ok(Ok(reranker)) => reranker,
            Ok(Err(_recv_err)) => Err(VectorCodeError::RerankerError {
                message: "ONNX session creation channel closed unexpectedly".to_string(),
            }),
            Err(_elapsed) => Err(VectorCodeError::RerankerError {
                message: reranker_timeout_error_message(),
            }),
        }
    }

    /// Encode a query-document pair for cross-encoder input.
    ///
    /// Uses the tokenizer's pair encoding to produce `[CLS] query [SEP] doc [SEP]`.
    /// Truncates to `MAX_TOKENS` if necessary.
    ///
    /// Returns `(input_ids, attention_mask)`.
    /// XLM-RoBERTa (BGE-Reranker base) does NOT use `token_type_ids` —
    /// passing them would cause "Invalid input name" at inference time.
    fn encode_pair(&self, query: &str, doc: &str) -> RerankResult<(Vec<i64>, Vec<i64>)> {
        let encoding = self.tokenizer.encode((query, doc), true).map_err(|e| {
            VectorCodeError::RerankerError {
                message: format!("Reranker tokenization failed: {e}"),
            }
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

        Ok((ids, mask))
    }

    /// Apply the sigmoid function to map a logit to [0, 1].
    fn sigmoid(x: f32) -> f32 {
        1.0 / (1.0 + (-x).exp())
    }
}

/// Build the error message for ONNX session creation timeout.
pub fn reranker_timeout_error_message() -> String {
    "ONNX Runtime initialization timed out after 60s.\n\
     \n\
     This usually means the ONNX Runtime is hanging during initialization.\n\
     \n\
     Fix options:\n\
       • Set ORT_DISABLE_COREML=1 before running (CoreML EP can sometimes hang on macOS)\n\
       • Or run: vectorcode init"
        .to_string()
}

// ── RerankerModelManager ──────────────────────────────────────────────────────

/// Manages BGE-Reranker-v2-m3 model download, caching, and loading.
///
/// Wraps the embedder's `ModelManager` to reuse its `download_from` infrastructure,
/// but points at a different model directory and HuggingFace repo.
struct RerankerModelManager {
    inner: ModelManager,
}

impl RerankerModelManager {
    /// Create a `RerankerModelManager` with the default model directory.
    ///
    /// Path: `~/.vectorcode/models/bge-reranker-v2-m3/`
    fn new() -> Self {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        let model_dir = PathBuf::from(home)
            .join(".vectorcode")
            .join("models")
            .join("bge-reranker-v2-m3");
        Self {
            inner: ModelManager::with_model_dir(model_dir),
        }
    }

    /// Create a `RerankerModelManager` with a custom model directory (for testing).
    fn with_model_dir(dir: PathBuf) -> Self {
        Self {
            inner: ModelManager::with_model_dir(dir),
        }
    }

    /// Check whether both `model.onnx` and `tokenizer.json` are present in the cache.
    fn is_downloaded(&self) -> bool {
        self.inner.is_downloaded()
    }

    /// Load the cached model and tokenizer bytes into memory.
    ///
    /// Returns `(model_bytes, tokenizer_bytes)`.
    ///
    /// # Errors
    /// Returns a `RerankerError` if the files are not cached.
    /// Callers should call `download_model()` first if the model is missing.
    fn load_model(&self) -> Result<(Vec<u8>, Vec<u8>), VectorCodeError> {
        if !self.is_downloaded() {
            return Err(VectorCodeError::RerankerError {
                message: format!(
                    "Reranker model not found at {}. \
                     Download it with the `vectorcode init` reranker step \
                     or place model.onnx + tokenizer.json manually.",
                    self.inner.model_dir().display()
                ),
            });
        }

        let model_bytes = std::fs::read(self.inner.model_dir().join(MODEL_FILENAME))?;
        let tokenizer_bytes = std::fs::read(self.inner.model_dir().join(TOKENIZER_FILENAME))?;

        Ok((model_bytes, tokenizer_bytes))
    }

    /// Download the BGE-Reranker ONNX model and tokenizer from HuggingFace.
    ///
    /// Reuses the embedder's `ModelManager::download_from` with reranker-specific URLs.
    pub(crate) async fn download_model(&self) -> Result<(), VectorCodeError> {
        let model_url = format!("{HF_BASE_URL}/{ONNX_MODEL_PATH}");
        let tokenizer_url = format!("{HF_BASE_URL}/{TOKENIZER_FILENAME}");
        self.inner.download_from(&model_url, &tokenizer_url).await
    }
}

// ── Reranker trait implementation ─────────────────────────────────────────────

#[async_trait]
impl Reranker for OnnxReranker {
    async fn rerank(
        &self,
        query: &str,
        documents: &[RerankDocument],
    ) -> RerankResult<Vec<(usize, f32)>> {
        if documents.is_empty() {
            return Ok(vec![]);
        }

        let batch_size = documents.len();

        // Encode all query-document pairs
        let mut all_ids = Vec::with_capacity(batch_size);
        let mut all_masks = Vec::with_capacity(batch_size);
        let mut max_len = 0usize;

        for doc in documents {
            let (ids, mask) = self.encode_pair(query, &doc.content)?;
            max_len = max_len.max(ids.len());
            all_ids.push(ids);
            all_masks.push(mask);
        }

        // Pad all sequences to max_len and flatten into contiguous buffers
        let mut flat_ids = Vec::with_capacity(batch_size * max_len);
        let mut flat_masks = Vec::with_capacity(batch_size * max_len);

        for i in 0..batch_size {
            let pad_len = max_len - all_ids[i].len();

            flat_ids.extend_from_slice(&all_ids[i]);
            flat_ids.extend(std::iter::repeat(0i64).take(pad_len));

            flat_masks.extend_from_slice(&all_masks[i]);
            flat_masks.extend(std::iter::repeat(0i64).take(pad_len));
        }

        // Create input tensors: shape [batch_size, max_len]
        let input_ids_tensor =
            Tensor::from_array(([batch_size, max_len], flat_ids.into_boxed_slice())).map_err(
                |e| VectorCodeError::RerankerError {
                    message: format!("Failed to create input_ids tensor: {e}"),
                },
            )?;

        let attention_mask_tensor =
            Tensor::from_array(([batch_size, max_len], flat_masks.into_boxed_slice())).map_err(
                |e| VectorCodeError::RerankerError {
                    message: format!("Failed to create attention_mask tensor: {e}"),
                },
            )?;

        // Run inference — scoped so lock is released promptly
        let scores: Vec<f32> = {
            let mut session = self.session.lock().await;
            let outputs = session
                .run(ort::inputs![
                    "input_ids" => input_ids_tensor,
                    "attention_mask" => attention_mask_tensor,
                ])
                .map_err(|e| VectorCodeError::RerankerError {
                    message: format!("ONNX reranker session run failed: {e}"),
                })?;

            // Probe output name dynamically (may be "logits", "output", etc.)
            let output_key =
                outputs
                    .keys()
                    .next()
                    .ok_or_else(|| VectorCodeError::RerankerError {
                        message: "ONNX reranker model produced no outputs".to_string(),
                    })?;

            let (_, output_data) = outputs
                .get(output_key)
                .ok_or_else(|| VectorCodeError::RerankerError {
                    message: format!(
                        "ONNX reranker output '{output_key}' not found. \
                         Available outputs: {:?}",
                        outputs.keys().collect::<Vec<_>>()
                    ),
                })?
                .try_extract_tensor::<f32>()
                .map_err(|e| VectorCodeError::RerankerError {
                    message: format!("Failed to extract reranker output tensor: {e}"),
                })?;

            // Output shape is [batch_size, 1] — extract one logit per document
            if output_data.len() < batch_size {
                return Err(VectorCodeError::RerankerError {
                    message: format!(
                        "ONNX reranker output has {} elements, expected at least {batch_size}",
                        output_data.len()
                    ),
                });
            }

            output_data
                .iter()
                .copied()
                .take(batch_size)
                .map(Self::sigmoid)
                .collect()
        }; // Lock released here

        // Pair each score with its original document index, sort descending
        let mut indexed_scores: Vec<(usize, f32)> = documents
            .iter()
            .enumerate()
            .map(|(i, doc)| (doc.index, scores[i]))
            .collect();

        indexed_scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        Ok(indexed_scores)
    }

    fn provider_name(&self) -> &str {
        "onnx"
    }

    fn model_name(&self) -> &str {
        Self::MODEL_NAME
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn onnx_reranker_metadata_constants() {
        assert_eq!(OnnxReranker::MODEL_NAME, "BGE-Reranker-v2-m3");
        assert_eq!(OnnxReranker::MAX_TOKENS, 8192);
    }

    #[test]
    fn onnx_reranker_new_fails_with_invalid_model() {
        let invalid_model = b"not a valid onnx model";
        let invalid_tokenizer = b"not a valid tokenizer";
        let result = OnnxReranker::new(invalid_model, invalid_tokenizer);
        assert!(result.is_err(), "Should fail with invalid model bytes");
    }

    #[test]
    fn onnx_reranker_new_fails_with_invalid_tokenizer() {
        let invalid_tokenizer = b"{}"; // Valid JSON but not a tokenizer
        let result = OnnxReranker::new(b"model", invalid_tokenizer);
        assert!(result.is_err(), "Should fail with invalid tokenizer");
    }

    #[test]
    fn from_cache_errors_when_model_not_downloaded() {
        let empty_dir = tempfile::tempdir().unwrap();
        let result = OnnxReranker::from_model_dir(empty_dir.path().to_path_buf());
        assert!(
            result.is_err(),
            "from_cache should fail when model files are missing"
        );
        let err_msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => unreachable!(),
        };
        assert!(
            err_msg.contains("Reranker model not found"),
            "Error should mention model not found, got: {err_msg}"
        );
    }

    /// The internal `from_reranker_model_manager_with_timeout` does NOT
    /// auto-download — it is the fast path used after download has already
    /// succeeded (or for testing with pre-seeded directories).
    #[tokio::test]
    async fn from_reranker_model_manager_with_timeout_errors_when_not_cached() {
        let empty_dir = tempfile::tempdir().unwrap();
        let manager = RerankerModelManager::with_model_dir(empty_dir.path().to_path_buf());
        let result = OnnxReranker::from_reranker_model_manager_with_timeout(&manager).await;
        assert!(
            result.is_err(),
            "internal timeout path should fail when model files are missing"
        );
        let err_msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => unreachable!(),
        };
        assert!(
            err_msg.contains("Reranker model not found"),
            "Error should mention model path, got: {err_msg}"
        );
    }

    /// `from_model_dir_with_timeout` also skips auto-download — it expects
    /// the directory to already contain the model files.
    #[tokio::test]
    async fn from_model_dir_with_timeout_errors_when_empty() {
        let empty_dir = tempfile::tempdir().unwrap();
        let result =
            OnnxReranker::from_model_dir_with_timeout(empty_dir.path().to_path_buf()).await;
        assert!(
            result.is_err(),
            "Should return error, not panic, when model is missing"
        );
    }

    #[test]
    fn sigmoid_maps_correctly() {
        // sigmoid(0) = 0.5
        assert!(
            (OnnxReranker::sigmoid(0.0) - 0.5).abs() < 1e-6,
            "sigmoid(0) should be 0.5"
        );

        // sigmoid(large positive) ≈ 1.0
        assert!(
            OnnxReranker::sigmoid(10.0) > 0.999,
            "sigmoid(10) should be close to 1.0"
        );

        // sigmoid(large negative) ≈ 0.0
        assert!(
            OnnxReranker::sigmoid(-10.0) < 0.001,
            "sigmoid(-10) should be close to 0.0"
        );

        // sigmoid is symmetric: sigmoid(x) + sigmoid(-x) = 1.0
        let x = 2.5;
        let sum = OnnxReranker::sigmoid(x) + OnnxReranker::sigmoid(-x);
        assert!(
            (sum - 1.0).abs() < 1e-6,
            "sigmoid(x) + sigmoid(-x) should be 1.0, got {sum}"
        );
    }

    #[test]
    fn timeout_error_message_contains_helpful_hints() {
        let msg = reranker_timeout_error_message();
        assert!(
            msg.contains("timed out"),
            "Error should mention 'timed out', got: {msg}"
        );
        assert!(
            msg.contains("vectorcode init"),
            "Error should mention 'vectorcode init', got: {msg}"
        );
    }

    #[test]
    fn from_cache_errors_with_invalid_model_bytes_in_cache() {
        let dir = tempfile::tempdir().unwrap();
        // Write fake model + invalid tokenizer to simulate corrupted cache
        std::fs::write(dir.path().join("model.onnx"), b"fake-model").unwrap();
        std::fs::write(dir.path().join("tokenizer.json"), b"not-json").unwrap();
        let result = OnnxReranker::from_model_dir(dir.path().to_path_buf());
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
        let result = OnnxReranker::from_model_dir(dir.path().to_path_buf());
        assert!(
            result.is_err(),
            "Should fail when tokenizer.json is missing"
        );
        let err_msg = match result {
            Err(e) => e.to_string(),
            Ok(_) => unreachable!(),
        };
        assert!(
            err_msg.contains("Reranker model not found"),
            "Error should mention model not found, got: {err_msg}"
        );
    }

    #[test]
    fn reranker_model_manager_default_dir_ends_with_expected_folder() {
        let manager = RerankerModelManager::new();
        let dir = manager.inner.model_dir();
        assert!(
            dir.to_str().unwrap().ends_with("bge-reranker-v2-m3"),
            "Expected path ending with 'bge-reranker-v2-m3', got: {}",
            dir.display()
        );
    }

    #[test]
    fn reranker_model_manager_is_downloaded_false_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let manager = RerankerModelManager::with_model_dir(dir.path().to_path_buf());
        assert!(!manager.is_downloaded());
    }

    #[test]
    fn reranker_model_manager_load_model_errors_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let manager = RerankerModelManager::with_model_dir(dir.path().to_path_buf());
        let result = manager.load_model();
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("Reranker model not found"),
            "Error should mention missing model, got: {err_msg}"
        );
    }
}
