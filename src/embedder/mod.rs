//! Embedder trait and provider implementations.
//!
//! The `Embedder` trait defines the interface for all embedding providers.
//! Providers implement this trait to generate vector embeddings from text.

pub mod gemini;
pub mod http;
pub mod mock;
pub mod model_manager;
pub mod ollama;
pub mod onnx;
pub mod openai;
pub mod openrouter;

use crate::error::VectorCodeError;
use async_trait::async_trait;

/// Result type for embedder operations.
pub type EmbedderResult<T> = Result<T, VectorCodeError>;

/// Trait for embedding text into vector representations.
///
/// All providers (ONNX, Gemini, Ollama, OpenAI) implement this trait.
/// The trait is object-safe via async-trait, allowing `Arc<dyn Embedder>`.
#[async_trait]
pub trait Embedder: Send + Sync {
    /// Generate embedding for a single text.
    async fn embed(&self, text: &str) -> EmbedderResult<Vec<f32>>;

    /// Generate embeddings for a batch of texts.
    ///
    /// Default implementation calls `embed()` sequentially.
    /// Providers with native batch support should override for efficiency.
    async fn embed_batch(&self, texts: &[&str]) -> EmbedderResult<Vec<Vec<f32>>> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    /// Number of dimensions in the output vectors.
    fn dimensions(&self) -> u32;

    /// Provider name for metadata (e.g., "onnx", "gemini").
    fn provider_name(&self) -> &str;

    /// Model identifier for metadata (e.g., "all-MiniLM-L6-v2").
    fn model_name(&self) -> &str;

    /// Maximum input token length supported.
    fn max_tokens(&self) -> u32;
}
