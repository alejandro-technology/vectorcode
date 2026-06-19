use async_trait::async_trait;

use crate::error::VectorCodeError;

pub mod onnx;

/// Result type alias for reranker operations.
pub type Result<T> = std::result::Result<T, VectorCodeError>;

/// Error type alias for reranker operations (re-exported for convenience).
pub type RerankError = VectorCodeError;

/// A document to be reranked, carrying its content and original index.
#[derive(Debug, Clone)]
pub struct RerankDocument {
    /// The text content of the document.
    pub content: String,
    /// The original position in the candidate list.
    pub index: usize,
}

/// Trait for reranking documents against a query.
///
/// Implementations score each document's relevance to the query and return
/// `(original_index, score)` pairs sorted by score in descending order.
#[async_trait]
pub trait Reranker: Send + Sync {
    /// Re-score documents against the query.
    ///
    /// Returns a vector of `(original_index, relevance_score)` tuples sorted
    /// by relevance score in descending order. Indices refer to positions in
    /// the input `documents` slice.
    async fn rerank(&self, query: &str, documents: &[RerankDocument]) -> Result<Vec<(usize, f32)>>;

    /// Returns the name of the reranker provider (e.g., "onnx").
    fn provider_name(&self) -> &str;

    /// Returns the model name (e.g., "BGE-Reranker-v2-m3").
    fn model_name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rerank_document_fields_accessible() {
        let doc = RerankDocument {
            content: "hello world".to_string(),
            index: 3,
        };
        assert_eq!(doc.content, "hello world");
        assert_eq!(doc.index, 3);
    }

    #[test]
    fn rerank_document_clone_and_debug() {
        let doc = RerankDocument {
            content: "test".to_string(),
            index: 0,
        };
        let cloned = doc.clone();
        assert_eq!(cloned.content, "test");
        assert_eq!(cloned.index, 0);
        // Debug should not panic
        let _ = format!("{:?}", doc);
    }
}
