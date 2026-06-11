//! Mock embedder for testing — returns deterministic vectors based on input hash.
//!
//! This is NOT a real embedding model. It produces reproducible vectors for
//! testing the indexing pipeline without requiring ONNX runtime or API keys.

use crate::embedder::{Embedder, EmbedderResult};
use async_trait::async_trait;

/// Mock embedder that generates deterministic vectors from text hashes.
pub struct MockEmbedder {
    dims: u32,
}

impl MockEmbedder {
    /// Create a new MockEmbedder with the specified dimensions.
    pub fn new(dims: u32) -> Self {
        Self { dims }
    }

    /// Generate a deterministic vector from text using a simple hash-based approach.
    /// The vector is L2-normalized (unit length).
    fn generate_vector(&self, text: &str) -> Vec<f32> {
        let dims = self.dims as usize;
        let mut vector = vec![0.0f32; dims];

        // Use a simple hash to generate deterministic values
        let mut hash: u64 = 0;
        for byte in text.bytes() {
            hash = hash.wrapping_mul(31).wrapping_add(byte as u64);
        }

        // Fill vector with deterministic values based on hash
        for (i, val) in vector.iter_mut().enumerate() {
            // Mix the hash with the index to get different values per dimension
            let mixed = hash.wrapping_mul(37).wrapping_add(i as u64);
            // Convert to f32 in range [-1, 1]
            *val = ((mixed % 1000) as f32 - 500.0) / 500.0;
        }

        // L2 normalize
        let norm: f32 = vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm > 0.0 {
            for val in vector.iter_mut() {
                *val /= norm;
            }
        }

        vector
    }
}

#[async_trait]
impl Embedder for MockEmbedder {
    async fn embed(&self, text: &str) -> EmbedderResult<Vec<f32>> {
        Ok(self.generate_vector(text))
    }

    fn dimensions(&self) -> u32 {
        self.dims
    }

    fn provider_name(&self) -> &str {
        "mock"
    }

    fn model_name(&self) -> &str {
        "mock-embedder"
    }

    fn max_tokens(&self) -> u32 {
        512
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_embedder_returns_correct_dimensions() {
        let embedder = MockEmbedder::new(384);
        let vector = embedder.embed("test input").await.unwrap();
        assert_eq!(vector.len(), 384, "Expected 384 dimensions");
    }

    #[tokio::test]
    async fn mock_embedder_is_deterministic() {
        let embedder = MockEmbedder::new(128);
        let v1 = embedder.embed("hello world").await.unwrap();
        let v2 = embedder.embed("hello world").await.unwrap();
        assert_eq!(v1, v2, "Same input should produce same vector");
    }

    #[tokio::test]
    async fn mock_embedder_different_inputs_different_vectors() {
        let embedder = MockEmbedder::new(128);
        let v1 = embedder.embed("hello").await.unwrap();
        let v2 = embedder.embed("world").await.unwrap();
        assert_ne!(v1, v2, "Different inputs should produce different vectors");
    }

    #[tokio::test]
    async fn mock_embedder_vector_is_l2_normalized() {
        let embedder = MockEmbedder::new(256);
        let vector = embedder.embed("test normalization").await.unwrap();
        let sum_squares: f32 = vector.iter().map(|x| x * x).sum();
        let norm = sum_squares.sqrt();
        assert!(
            (norm - 1.0).abs() < 1e-5,
            "Vector should be L2-normalized, got norm={norm}"
        );
    }

    #[tokio::test]
    async fn mock_embedder_batch_returns_correct_count() {
        let embedder = MockEmbedder::new(64);
        let texts = vec!["first", "second", "third"];
        let vectors = embedder.embed_batch(&texts).await.unwrap();
        assert_eq!(vectors.len(), 3, "Should return one vector per input");
        for v in &vectors {
            assert_eq!(v.len(), 64, "Each vector should have correct dimensions");
        }
    }

    #[tokio::test]
    async fn mock_embedder_batch_empty_input() {
        let embedder = MockEmbedder::new(64);
        let texts: Vec<&str> = vec![];
        let vectors = embedder.embed_batch(&texts).await.unwrap();
        assert_eq!(vectors.len(), 0, "Empty input should return empty output");
    }

    #[test]
    fn mock_embedder_metadata_methods() {
        let embedder = MockEmbedder::new(768);
        assert_eq!(embedder.dimensions(), 768);
        assert_eq!(embedder.provider_name(), "mock");
        assert_eq!(embedder.model_name(), "mock-embedder");
        assert_eq!(embedder.max_tokens(), 512);
    }

    #[tokio::test]
    async fn mock_embedder_different_dimensions() {
        let e384 = MockEmbedder::new(384);
        let e768 = MockEmbedder::new(768);
        let v384 = e384.embed("test").await.unwrap();
        let v768 = e768.embed("test").await.unwrap();
        assert_eq!(v384.len(), 384);
        assert_eq!(v768.len(), 768);
    }
}
