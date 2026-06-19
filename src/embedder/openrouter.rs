//! OpenRouter embedding provider.
//!
//! Uses OpenRouter's API for text embeddings via an OpenAI-compatible endpoint.
//! Default model: nvidia/llama-nemotron-embed-vl-1b-v2:free (768 dimensions).
//! Spec §7.2: OpenRouter

use crate::embedder::http::{
    calculate_backoff, jitter_factor, read_retry_after, should_retry, BASE_BACKOFF_MS,
    MAX_BACKOFF_MS, MAX_RETRIES,
};
use crate::embedder::{Embedder, EmbedderResult};
use crate::error::VectorCodeError;
use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Maximum items per batch request (OpenRouter API limit).
const OPENROUTER_BATCH_SIZE: usize = 2048;

/// OpenRouter embedding provider.
///
/// Sends text to OpenRouter's embeddings API (OpenAI-compatible) and returns
/// vector embeddings. Requires an API key set via `OPENROUTER_API_KEY` env var
/// or config.
#[derive(Debug)]
pub struct OpenRouterEmbedder {
    model: String,
    api_key: SecretString,
    client: reqwest::Client,
}

impl OpenRouterEmbedder {
    /// Default model identifier.
    pub const DEFAULT_MODEL: &'static str = "nvidia/llama-nemotron-embed-vl-1b-v2:free";
    /// Output dimensions for the default model.
    pub const DEFAULT_DIMENSIONS: u32 = 768;
    /// Maximum input token length.
    pub const MAX_TOKENS: u32 = 8191;
    /// API endpoint URL.
    pub const API_URL: &'static str = "https://openrouter.ai/api/v1/embeddings";

    /// Create a new OpenRouterEmbedder.
    ///
    /// # Errors
    /// Returns `ApiKeyMissing` if `api_key` is empty.
    pub fn new(api_key: String) -> EmbedderResult<Self> {
        Self::with_client(api_key, crate::embedder::http::build_http_client()?)
    }

    /// Create with a custom model name.
    pub fn with_model(api_key: String, model: String) -> EmbedderResult<Self> {
        Self::with_client_and_model(api_key, model, crate::embedder::http::build_http_client()?)
    }

    /// Create with a custom reqwest::Client (useful for testing).
    pub fn with_client(api_key: String, client: reqwest::Client) -> EmbedderResult<Self> {
        Self::with_client_and_model(api_key, Self::DEFAULT_MODEL.to_string(), client)
    }

    /// Create with custom client and model.
    pub fn with_client_and_model(
        api_key: String,
        model: String,
        client: reqwest::Client,
    ) -> EmbedderResult<Self> {
        if api_key.is_empty() {
            return Err(VectorCodeError::ApiKeyMissing {
                env_var: "OPENROUTER_API_KEY".to_string(),
            });
        }
        Ok(Self {
            model,
            api_key: SecretString::from(api_key),
            client,
        })
    }

    /// Build the embeddings endpoint URL.
    fn embed_url(&self) -> &str {
        Self::API_URL
    }

    /// Build request body for embed (works for both single and batch).
    fn build_request(&self, texts: &[&str]) -> OpenRouterRequest {
        OpenRouterRequest {
            model: self.model.clone(),
            input: texts.iter().map(|t| t.to_string()).collect(),
        }
    }

    /// Parse embed response JSON into vectors.
    ///
    /// Results are sorted by `index` to guarantee input order.
    fn parse_response(body: &str) -> EmbedderResult<Vec<Vec<f32>>> {
        let response: OpenRouterResponse =
            serde_json::from_str(body).map_err(|e| VectorCodeError::EmbedderError {
                message: format!("Failed to parse OpenRouter response: {e}"),
            })?;
        let mut data = response.data;
        data.sort_by_key(|d| d.index);
        Ok(data.into_iter().map(|d| d.embedding).collect())
    }
}

/// OpenRouter embed request body (OpenAI-compatible format).
#[derive(Serialize)]
struct OpenRouterRequest {
    model: String,
    input: Vec<String>,
}

/// OpenRouter embed response body (OpenAI-compatible format).
#[derive(Deserialize)]
struct OpenRouterResponse {
    data: Vec<OpenRouterEmbeddingData>,
}

/// Single embedding entry in the OpenRouter response.
#[derive(Deserialize)]
struct OpenRouterEmbeddingData {
    embedding: Vec<f32>,
    index: usize,
}

#[async_trait]
impl Embedder for OpenRouterEmbedder {
    async fn embed(&self, text: &str) -> EmbedderResult<Vec<f32>> {
        let max_len = Self::MAX_TOKENS as usize * 4;
        if text.len() > max_len {
            return Err(VectorCodeError::EmbedderError {
                message: format!(
                    "Text length ({} bytes) exceeds maximum limit of {} bytes",
                    text.len(),
                    max_len
                ),
            });
        }
        let url = self.embed_url();
        let body = self.build_request(&[text]);

        for attempt in 0..=MAX_RETRIES {
            let response = self
                .client
                .post(url)
                .bearer_auth(self.api_key.expose_secret())
                .json(&body)
                .send()
                .await
                .map_err(|e| VectorCodeError::EmbedderError {
                    message: format!("OpenRouter HTTP request failed: {e}"),
                })?;

            let status = response.status().as_u16();

            if response.status().is_success() {
                let response_body =
                    response
                        .text()
                        .await
                        .map_err(|e| VectorCodeError::EmbedderError {
                            message: format!("Failed to read OpenRouter response body: {e}"),
                        })?;
                let vectors = Self::parse_response(&response_body)?;
                return vectors
                    .into_iter()
                    .next()
                    .ok_or_else(|| VectorCodeError::EmbedderError {
                        message: "OpenRouter returned empty data array".to_string(),
                    });
            }

            if should_retry(status) {
                if attempt < MAX_RETRIES {
                    let backoff = calculate_backoff(
                        attempt,
                        BASE_BACKOFF_MS,
                        MAX_BACKOFF_MS,
                        jitter_factor(),
                    );
                    let effective = if status == 429 {
                        read_retry_after(&response).unwrap_or(backoff.max(Duration::from_secs(30)))
                    } else {
                        backoff
                    };
                    tokio::time::sleep(effective).await;
                    continue;
                }
                return Err(VectorCodeError::EmbedderError {
                    message: format!(
                        "OpenRouter API: max retries ({MAX_RETRIES}) exceeded (HTTP {status})"
                    ),
                });
            }

            let response_body = response.text().await.unwrap_or_default();
            return Err(VectorCodeError::EmbedderError {
                message: format!("OpenRouter API error (HTTP {status}): {response_body}"),
            });
        }

        Err(VectorCodeError::EmbedderError {
            message: "OpenRouter: max retries exceeded".to_string(),
        })
    }

    async fn embed_batch(&self, texts: &[&str]) -> EmbedderResult<Vec<Vec<f32>>> {
        let max_len = Self::MAX_TOKENS as usize * 4;
        for (i, text) in texts.iter().enumerate() {
            if text.len() > max_len {
                return Err(VectorCodeError::EmbedderError {
                    message: format!(
                        "Text at index {} length ({} bytes) exceeds maximum limit of {} bytes",
                        i,
                        text.len(),
                        max_len
                    ),
                });
            }
        }
        let url = self.embed_url();
        let mut all_embeddings = Vec::with_capacity(texts.len());

        for chunk in texts.chunks(OPENROUTER_BATCH_SIZE) {
            let body = self.build_request(chunk);

            for attempt in 0..=MAX_RETRIES {
                let response = self
                    .client
                    .post(url)
                    .bearer_auth(self.api_key.expose_secret())
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| VectorCodeError::EmbedderError {
                        message: format!("OpenRouter batch HTTP request failed: {e}"),
                    })?;

                let status = response.status().as_u16();

                if response.status().is_success() {
                    let response_body =
                        response
                            .text()
                            .await
                            .map_err(|e| VectorCodeError::EmbedderError {
                                message: format!(
                                    "Failed to read OpenRouter batch response body: {e}"
                                ),
                            })?;
                    let mut batch_vectors = Self::parse_response(&response_body)?;
                    all_embeddings.append(&mut batch_vectors);
                    break;
                }

                if should_retry(status) {
                    if attempt < MAX_RETRIES {
                        let backoff = calculate_backoff(
                            attempt,
                            BASE_BACKOFF_MS,
                            MAX_BACKOFF_MS,
                            jitter_factor(),
                        );
                        let effective = if status == 429 {
                            read_retry_after(&response)
                                .unwrap_or(backoff.max(Duration::from_secs(30)))
                        } else {
                            backoff
                        };
                        tokio::time::sleep(effective).await;
                        continue;
                    }
                    return Err(VectorCodeError::EmbedderError {
                        message: format!(
                            "OpenRouter batch API: max retries ({MAX_RETRIES}) exceeded (HTTP {status})"
                        ),
                    });
                }

                let response_body = response.text().await.unwrap_or_default();
                return Err(VectorCodeError::EmbedderError {
                    message: format!("OpenRouter batch API error (HTTP {status}): {response_body}"),
                });
            }
        }

        Ok(all_embeddings)
    }

    fn dimensions(&self) -> u32 {
        Self::DEFAULT_DIMENSIONS
    }

    fn provider_name(&self) -> &str {
        "openrouter"
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
    fn openrouter_new_fails_without_api_key() {
        let result = OpenRouterEmbedder::new(String::new());
        assert!(result.is_err(), "Empty API key should fail");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("OPENROUTER_API_KEY"),
            "Error should mention env var, got: {msg}"
        );
    }

    #[test]
    fn openrouter_new_succeeds_with_valid_key() {
        let embedder = OpenRouterEmbedder::new("sk-or-test-key".to_string());
        assert!(embedder.is_ok(), "Valid API key should succeed");
    }

    #[test]
    fn openrouter_metadata_methods() {
        let embedder = OpenRouterEmbedder::new("sk-or-test".to_string()).unwrap();
        assert_eq!(embedder.provider_name(), "openrouter");
        assert_eq!(
            embedder.model_name(),
            "nvidia/llama-nemotron-embed-vl-1b-v2:free"
        );
        assert_eq!(embedder.dimensions(), 768);
        assert_eq!(embedder.max_tokens(), 8191);
    }

    #[test]
    fn openrouter_custom_model() {
        let embedder =
            OpenRouterEmbedder::with_model("sk-or-test".to_string(), "custom-model".to_string())
                .unwrap();
        assert_eq!(embedder.model_name(), "custom-model");
    }

    #[test]
    fn openrouter_embed_url_construction() {
        let embedder = OpenRouterEmbedder::new("sk-or-test".to_string()).unwrap();
        let url = embedder.embed_url();
        assert_eq!(url, "https://openrouter.ai/api/v1/embeddings");
    }

    #[test]
    fn openrouter_request_body_format() {
        let embedder = OpenRouterEmbedder::new("sk-or-test".to_string()).unwrap();
        let body = embedder.build_request(&["hello world"]);
        assert_eq!(body.model, "nvidia/llama-nemotron-embed-vl-1b-v2:free");
        assert_eq!(body.input, vec!["hello world"]);
    }

    #[test]
    fn openrouter_batch_request_body_multiple_inputs() {
        let embedder = OpenRouterEmbedder::new("sk-or-test".to_string()).unwrap();
        let texts = vec!["chunk one", "chunk two"];
        let body = embedder.build_request(&texts);
        assert_eq!(body.input.len(), 2, "Should include all inputs");
        assert_eq!(body.input[0], "chunk one");
        assert_eq!(body.input[1], "chunk two");
        assert_eq!(body.model, "nvidia/llama-nemotron-embed-vl-1b-v2:free");
    }

    #[test]
    fn openrouter_parse_response_success() {
        let json = r#"{
            "data": [
                {"embedding": [0.1, 0.2, 0.3], "index": 0},
                {"embedding": [0.4, 0.5, 0.6], "index": 1}
            ],
            "model": "nvidia/llama-nemotron-embed-vl-1b-v2:free",
            "usage": {"prompt_tokens": 5, "total_tokens": 5}
        }"#;
        let result = OpenRouterEmbedder::parse_response(json).unwrap();
        assert_eq!(result.len(), 2, "Should parse 2 embedding vectors");
        assert_eq!(result[0].len(), 3, "First vector should have 3 dims");
        assert!((result[0][0] - 0.1).abs() < 1e-6);
        assert!((result[1][2] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn openrouter_parse_response_sorts_by_index() {
        // Response with out-of-order indices
        let json = r#"{
            "data": [
                {"embedding": [0.4, 0.5], "index": 1},
                {"embedding": [0.1, 0.2], "index": 0}
            ],
            "model": "nvidia/llama-nemotron-embed-vl-1b-v2:free"
        }"#;
        let result = OpenRouterEmbedder::parse_response(json).unwrap();
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
    fn openrouter_parse_response_invalid_json() {
        let result = OpenRouterEmbedder::parse_response("not json at all");
        assert!(result.is_err(), "Invalid JSON should fail");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("parse"), "Error should mention parsing: {msg}");
    }

    #[test]
    fn openrouter_constants() {
        assert_eq!(
            OpenRouterEmbedder::DEFAULT_MODEL,
            "nvidia/llama-nemotron-embed-vl-1b-v2:free"
        );
        assert_eq!(OpenRouterEmbedder::DEFAULT_DIMENSIONS, 768);
        assert_eq!(OpenRouterEmbedder::MAX_TOKENS, 8191);
        assert_eq!(
            OpenRouterEmbedder::API_URL,
            "https://openrouter.ai/api/v1/embeddings"
        );
    }
}
