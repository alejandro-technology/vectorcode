//! OpenAI embedding provider.
//!
//! Uses OpenAI's API for text embeddings.
//! Default model: text-embedding-3-small (1536 dimensions, 8191 max tokens).
//! Spec §7.2: OpenAI

use crate::embedder::http::{
    calculate_backoff, jitter_factor, should_retry, BASE_BACKOFF_MS, MAX_BACKOFF_MS, MAX_RETRIES,
};
use crate::embedder::{Embedder, EmbedderResult};
use crate::error::VectorCodeError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Maximum items per batch request (OpenAI API limit).
const OPENAI_BATCH_SIZE: usize = 2048;

/// OpenAI embedding provider.
///
/// Sends text to OpenAI's embeddings API and returns vector embeddings.
/// Requires an API key set via `OPENAI_API_KEY` env var or config.
#[derive(Debug)]
pub struct OpenAiEmbedder {
    model: String,
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiEmbedder {
    /// Default model identifier.
    pub const DEFAULT_MODEL: &'static str = "text-embedding-3-small";
    /// Output dimensions for the default model.
    pub const DEFAULT_DIMENSIONS: u32 = 1536;
    /// Maximum input token length.
    pub const MAX_TOKENS: u32 = 8191;
    /// API endpoint URL.
    pub const API_URL: &'static str = "https://api.openai.com/v1/embeddings";

    /// Create a new OpenAiEmbedder.
    ///
    /// # Errors
    /// Returns `ApiKeyMissing` if `api_key` is empty.
    pub fn new(api_key: String) -> EmbedderResult<Self> {
        Self::with_client(api_key, reqwest::Client::new())
    }

    /// Create with a custom reqwest::Client (useful for testing).
    pub fn with_client(api_key: String, client: reqwest::Client) -> EmbedderResult<Self> {
        if api_key.is_empty() {
            return Err(VectorCodeError::ApiKeyMissing {
                env_var: "OPENAI_API_KEY".to_string(),
            });
        }
        Ok(Self {
            model: Self::DEFAULT_MODEL.to_string(),
            api_key,
            client,
        })
    }

    /// Build the embeddings endpoint URL.
    fn embed_url(&self) -> &str {
        Self::API_URL
    }

    /// Build request body for embed (works for both single and batch).
    fn build_request(&self, texts: &[&str]) -> OpenAiRequest {
        OpenAiRequest {
            model: self.model.clone(),
            input: texts.iter().map(|t| t.to_string()).collect(),
        }
    }

    /// Parse embed response JSON into vectors.
    ///
    /// Results are sorted by `index` to guarantee input order.
    fn parse_response(body: &str) -> EmbedderResult<Vec<Vec<f32>>> {
        let response: OpenAiResponse =
            serde_json::from_str(body).map_err(|e| VectorCodeError::EmbedderError {
                message: format!("Failed to parse OpenAI response: {e}"),
            })?;
        let mut data = response.data;
        data.sort_by_key(|d| d.index);
        Ok(data.into_iter().map(|d| d.embedding).collect())
    }
}

/// OpenAI embed request body.
#[derive(Serialize)]
struct OpenAiRequest {
    model: String,
    input: Vec<String>,
}

/// OpenAI embed response body.
#[derive(Deserialize)]
struct OpenAiResponse {
    data: Vec<OpenAiEmbeddingData>,
}

/// Single embedding entry in the OpenAI response.
#[derive(Deserialize)]
struct OpenAiEmbeddingData {
    embedding: Vec<f32>,
    index: usize,
}

#[async_trait]
impl Embedder for OpenAiEmbedder {
    async fn embed(&self, text: &str) -> EmbedderResult<Vec<f32>> {
        let url = self.embed_url();
        let body = self.build_request(&[text]);

        for attempt in 0..=MAX_RETRIES {
            let response = self
                .client
                .post(url)
                .bearer_auth(&self.api_key)
                .json(&body)
                .send()
                .await
                .map_err(|e| VectorCodeError::EmbedderError {
                    message: format!("OpenAI HTTP request failed: {e}"),
                })?;

            let status = response.status().as_u16();

            if response.status().is_success() {
                let response_body =
                    response
                        .text()
                        .await
                        .map_err(|e| VectorCodeError::EmbedderError {
                            message: format!("Failed to read OpenAI response body: {e}"),
                        })?;
                let vectors = Self::parse_response(&response_body)?;
                return vectors
                    .into_iter()
                    .next()
                    .ok_or_else(|| VectorCodeError::EmbedderError {
                        message: "OpenAI returned empty data array".to_string(),
                    });
            }

            if should_retry(status) && attempt < MAX_RETRIES {
                let backoff =
                    calculate_backoff(attempt, BASE_BACKOFF_MS, MAX_BACKOFF_MS, jitter_factor());
                tokio::time::sleep(backoff).await;
                continue;
            }

            let response_body = response.text().await.unwrap_or_default();
            return Err(VectorCodeError::EmbedderError {
                message: format!("OpenAI API error (HTTP {status}): {response_body}"),
            });
        }

        Err(VectorCodeError::EmbedderError {
            message: "OpenAI: max retries exceeded".to_string(),
        })
    }

    async fn embed_batch(&self, texts: &[&str]) -> EmbedderResult<Vec<Vec<f32>>> {
        let url = self.embed_url();
        let mut all_embeddings = Vec::with_capacity(texts.len());

        for chunk in texts.chunks(OPENAI_BATCH_SIZE) {
            let body = self.build_request(chunk);

            for attempt in 0..=MAX_RETRIES {
                let response = self
                    .client
                    .post(url)
                    .bearer_auth(&self.api_key)
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| VectorCodeError::EmbedderError {
                        message: format!("OpenAI batch HTTP request failed: {e}"),
                    })?;

                let status = response.status().as_u16();

                if response.status().is_success() {
                    let response_body =
                        response
                            .text()
                            .await
                            .map_err(|e| VectorCodeError::EmbedderError {
                                message: format!("Failed to read OpenAI batch response body: {e}"),
                            })?;
                    let mut batch_vectors = Self::parse_response(&response_body)?;
                    all_embeddings.append(&mut batch_vectors);
                    break;
                }

                if should_retry(status) && attempt < MAX_RETRIES {
                    let backoff = calculate_backoff(
                        attempt,
                        BASE_BACKOFF_MS,
                        MAX_BACKOFF_MS,
                        jitter_factor(),
                    );
                    tokio::time::sleep(backoff).await;
                    continue;
                }

                let response_body = response.text().await.unwrap_or_default();
                return Err(VectorCodeError::EmbedderError {
                    message: format!("OpenAI batch API error (HTTP {status}): {response_body}"),
                });
            }
        }

        Ok(all_embeddings)
    }

    fn dimensions(&self) -> u32 {
        Self::DEFAULT_DIMENSIONS
    }

    fn provider_name(&self) -> &str {
        "openai"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn max_tokens(&self) -> u32 {
        Self::MAX_TOKENS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_new_fails_without_api_key() {
        let result = OpenAiEmbedder::new(String::new());
        assert!(result.is_err(), "Empty API key should fail");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("OPENAI_API_KEY"),
            "Error should mention env var, got: {msg}"
        );
    }

    #[test]
    fn openai_new_succeeds_with_valid_key() {
        let embedder = OpenAiEmbedder::new("sk-test-key".to_string());
        assert!(embedder.is_ok(), "Valid API key should succeed");
    }

    #[test]
    fn openai_metadata_methods() {
        let embedder = OpenAiEmbedder::new("sk-test".to_string()).unwrap();
        assert_eq!(embedder.provider_name(), "openai");
        assert_eq!(embedder.model_name(), "text-embedding-3-small");
        assert_eq!(embedder.dimensions(), 1536);
        assert_eq!(embedder.max_tokens(), 8191);
    }

    #[test]
    fn openai_embed_url_construction() {
        let embedder = OpenAiEmbedder::new("sk-test".to_string()).unwrap();
        let url = embedder.embed_url();
        assert_eq!(url, "https://api.openai.com/v1/embeddings");
    }

    #[test]
    fn openai_request_body_format() {
        let embedder = OpenAiEmbedder::new("sk-test".to_string()).unwrap();
        let body = embedder.build_request(&["hello world"]);
        assert_eq!(body.model, "text-embedding-3-small");
        assert_eq!(body.input, vec!["hello world"]);
    }

    #[test]
    fn openai_batch_request_body_multiple_inputs() {
        let embedder = OpenAiEmbedder::new("sk-test".to_string()).unwrap();
        let texts = vec!["chunk one", "chunk two"];
        let body = embedder.build_request(&texts);
        assert_eq!(body.input.len(), 2, "Should include all inputs");
        assert_eq!(body.input[0], "chunk one");
        assert_eq!(body.input[1], "chunk two");
        assert_eq!(body.model, "text-embedding-3-small");
    }

    #[test]
    fn openai_parse_response_success() {
        let json = r#"{
            "data": [
                {"embedding": [0.1, 0.2, 0.3], "index": 0},
                {"embedding": [0.4, 0.5, 0.6], "index": 1}
            ],
            "model": "text-embedding-3-small",
            "usage": {"prompt_tokens": 5, "total_tokens": 5}
        }"#;
        let result = OpenAiEmbedder::parse_response(json).unwrap();
        assert_eq!(result.len(), 2, "Should parse 2 embedding vectors");
        assert_eq!(result[0].len(), 3, "First vector should have 3 dims");
        assert!((result[0][0] - 0.1).abs() < 1e-6);
        assert!((result[1][2] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn openai_parse_response_sorts_by_index() {
        // Response with out-of-order indices
        let json = r#"{
            "data": [
                {"embedding": [0.4, 0.5], "index": 1},
                {"embedding": [0.1, 0.2], "index": 0}
            ],
            "model": "text-embedding-3-small"
        }"#;
        let result = OpenAiEmbedder::parse_response(json).unwrap();
        assert_eq!(result.len(), 2);
        assert!(
            (result[0][0] - 0.1).abs() < 1e-6,
            "First result should be index 0 (values [0.1, 0.2])"
        );
        assert!(
            (result[1][0] - 0.4).abs() < 1e-6,
            "Second result should be index 1 (values [0.4, 0.5])"
        );
    }

    #[test]
    fn openai_parse_response_invalid_json() {
        let result = OpenAiEmbedder::parse_response("not json at all");
        assert!(result.is_err(), "Invalid JSON should fail");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("parse"), "Error should mention parsing: {msg}");
    }

    #[test]
    fn openai_constants() {
        assert_eq!(OpenAiEmbedder::DEFAULT_MODEL, "text-embedding-3-small");
        assert_eq!(OpenAiEmbedder::DEFAULT_DIMENSIONS, 1536);
        assert_eq!(OpenAiEmbedder::MAX_TOKENS, 8191);
        assert_eq!(
            OpenAiEmbedder::API_URL,
            "https://api.openai.com/v1/embeddings"
        );
    }
}
