//! ONNX Runtime embedding provider.
//!
//! Uses the `ort` crate for ONNX Runtime and `tokenizers` for HuggingFace tokenization.
//! Default model: all-MiniLM-L6-v2 (384 dimensions, 512 max tokens).

use crate::embedder::{Embedder, EmbedderResult};
use crate::error::VectorCodeError;
use async_trait::async_trait;
use ort::session::Session;
use ort::value::Tensor;
use std::sync::Mutex;
use tokenizers::Tokenizer;

/// ONNX Runtime embedding provider.
///
/// Loads a quantized all-MiniLM-L6-v2 model and produces 384-dimensional vectors.
/// The model and tokenizer are loaded from memory (bundled via include_bytes!).
pub struct OnnxEmbedder {
    session: Mutex<Session>,
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

        let session = builder.commit_from_memory(model_bytes).map_err(|e| {
            VectorCodeError::EmbedderError {
                message: format!("Failed to load ONNX model: {e}"),
            }
        })?;

        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
        })
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

        // Run session with named inputs
        let mut session = self
            .session
            .lock()
            .map_err(|e| VectorCodeError::EmbedderError {
                message: format!("Failed to acquire session lock: {e}"),
            })?;
        let outputs = session
            .run(ort::inputs![
                "input_ids" => input_ids_tensor,
                "attention_mask" => attention_mask_tensor,
                "token_type_ids" => token_type_ids_tensor,
            ])
            .map_err(|e| VectorCodeError::EmbedderError {
                message: format!("ONNX session run failed: {e}"),
            })?;

        // Extract last_hidden_state — shape [1, seq_len, dims]
        let (_, output_data) = outputs["last_hidden_state"]
            .try_extract_tensor::<f32>()
            .map_err(|e| VectorCodeError::EmbedderError {
                message: format!("Failed to extract output tensor: {e}"),
            })?;

        // Apply mean pooling and normalize
        Ok(Self::pooling_and_normalize(
            output_data,
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
