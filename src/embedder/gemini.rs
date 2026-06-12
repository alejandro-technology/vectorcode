//! Gemini embedding provider.
//!
//! Uses Google's Gemini API for text embeddings.
//! Default model: gemini-embedding-001 (768 dimensions, Matryoshka-capable).
//! Spec §7.2: Gemini

use crate::embedder::http::{
    calculate_backoff, jitter_factor, should_retry, BASE_BACKOFF_MS, MAX_BACKOFF_MS, MAX_RETRIES,
};
use crate::embedder::{Embedder, EmbedderResult};
use crate::error::VectorCodeError;
use async_trait::async_trait;
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use std::time::Duration;

/// Maximum items per batch request (Gemini API limit).
const GEMINI_BATCH_SIZE: usize = 100;

/// Valid Matryoshka dimensions for Gemini embeddings.
const VALID_DIMENSIONS: &[u32] = &[256, 512, 768, 1024, 3072];

/// Gemini embedding provider.
///
/// Sends text to Google's Gemini API and returns vector embeddings.
/// Supports Matryoshka representation learning for configurable dimensions.
#[derive(Debug)]
pub struct GeminiEmbedder {
    model: String,
    dimensions: u32,
    api_key: SecretString,
    client: reqwest::Client,
}

impl GeminiEmbedder {
    /// Default model identifier.
    pub const DEFAULT_MODEL: &'static str = "gemini-embedding-001";
    /// Default output dimensions.
    pub const DEFAULT_DIMENSIONS: u32 = 768;
    /// Maximum input token length.
    pub const MAX_TOKENS: u32 = 2048;
    /// Base URL for the Gemini API.
    pub const BASE_URL: &'static str = "https://generativelanguage.googleapis.com/v1beta";

    /// Create a new GeminiEmbedder with a default reqwest::Client.
    ///
    /// # Errors
    /// - `ApiKeyMissing` if `api_key` is empty
    /// - `EmbedderError` if `dimensions` is not a valid Matryoshka size
    pub fn new(api_key: String, model: String, dimensions: u32) -> EmbedderResult<Self> {
        Self::with_client(
            api_key,
            model,
            dimensions,
            crate::embedder::http::build_http_client()?,
        )
    }

    /// Create with a custom reqwest::Client (useful for testing).
    pub fn with_client(
        api_key: String,
        model: String,
        dimensions: u32,
        client: reqwest::Client,
    ) -> EmbedderResult<Self> {
        if api_key.is_empty() {
            return Err(VectorCodeError::ApiKeyMissing {
                env_var: "GEMINI_API_KEY".to_string(),
            });
        }
        if !VALID_DIMENSIONS.contains(&dimensions) {
            return Err(VectorCodeError::EmbedderError {
                message: format!(
                    "Invalid Gemini dimensions {dimensions}. \
                     Valid Matryoshka sizes: {VALID_DIMENSIONS:?}"
                ),
            });
        }
        Ok(Self {
            model,
            dimensions,
            api_key: SecretString::from(api_key),
            client,
        })
    }

    /// Build the single-embed endpoint URL (no key in URL — uses header).
    fn embed_url(&self) -> String {
        format!("{}/models/{}:embedContent", Self::BASE_URL, self.model,)
    }

    /// Build the batch-embed endpoint URL (no key in URL — uses header).
    fn batch_url(&self) -> String {
        format!(
            "{}/models/{}:batchEmbedContents",
            Self::BASE_URL,
            self.model,
        )
    }

    /// Build request body for a single embed request.
    fn build_embed_request(&self, text: &str) -> serde_json::Value {
        serde_json::json!({
            "content": {
                "parts": [{ "text": text }]
            },
            "outputDimensionality": self.dimensions
        })
    }

    /// Build request body for a batch embed request.
    ///
    /// Each request must include its own `model` field (with `models/` prefix)
    /// per the Gemini batchEmbedContents API contract.
    fn build_batch_request(&self, texts: &[&str]) -> serde_json::Value {
        let model = format!("models/{}", self.model);
        let requests: Vec<serde_json::Value> = texts
            .iter()
            .map(|text| {
                serde_json::json!({
                    "model": model,
                    "content": { "parts": [{ "text": text }] },
                    "outputDimensionality": self.dimensions
                })
            })
            .collect();
        serde_json::json!({ "requests": requests })
    }

    /// Parse a single-embed response JSON into a vector.
    fn parse_embed_response(body: &str) -> EmbedderResult<Vec<f32>> {
        let response: GeminiEmbedResponse =
            serde_json::from_str(body).map_err(|e| VectorCodeError::EmbedderError {
                message: format!("Failed to parse Gemini embed response: {e}"),
            })?;
        Ok(response.embedding.values)
    }

    /// Parse a batch-embed response JSON into vectors.
    fn parse_batch_response(body: &str) -> EmbedderResult<Vec<Vec<f32>>> {
        let response: GeminiBatchResponse =
            serde_json::from_str(body).map_err(|e| VectorCodeError::EmbedderError {
                message: format!("Failed to parse Gemini batch response: {e}"),
            })?;
        Ok(response.embeddings.into_iter().map(|e| e.values).collect())
    }
}

/// Gemini single-embed response.
#[derive(Deserialize)]
struct GeminiEmbedResponse {
    embedding: GeminiEmbeddingValues,
}

/// Gemini embedding values container.
#[derive(Deserialize)]
struct GeminiEmbeddingValues {
    values: Vec<f32>,
}

/// Gemini batch-embed response.
#[derive(Deserialize)]
struct GeminiBatchResponse {
    embeddings: Vec<GeminiEmbeddingValues>,
}

/// Read the `Retry-After` header from an HTTP response, if present and parseable.
fn read_retry_after(response: &reqwest::Response) -> Option<Duration> {
    response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok())
        .map(Duration::from_secs)
}

#[async_trait]
impl Embedder for GeminiEmbedder {
    async fn embed(&self, text: &str) -> EmbedderResult<Vec<f32>> {
        let url = self.embed_url();
        let body = self.build_embed_request(text);

        for attempt in 0..=MAX_RETRIES {
            let response = self
                .client
                .post(&url)
                .header("x-goog-api-key", self.api_key.expose_secret())
                .json(&body)
                .send()
                .await
                .map_err(|e| VectorCodeError::EmbedderError {
                    message: format!("Gemini HTTP request failed: {e}"),
                })?;

            let status = response.status().as_u16();

            if response.status().is_success() {
                let response_body =
                    response
                        .text()
                        .await
                        .map_err(|e| VectorCodeError::EmbedderError {
                            message: format!("Failed to read Gemini response body: {e}"),
                        })?;
                return Self::parse_embed_response(&response_body);
            }

            if should_retry(status) {
                if attempt < MAX_RETRIES {
                    let backoff = calculate_backoff(
                        attempt,
                        BASE_BACKOFF_MS,
                        MAX_BACKOFF_MS,
                        jitter_factor(),
                    );
                    // If rate-limited, respect Retry-After header or use 30s floor
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
                        "Gemini API: max retries ({MAX_RETRIES}) exceeded (HTTP {status})"
                    ),
                });
            }

            let response_body = response.text().await.unwrap_or_default();
            return Err(VectorCodeError::EmbedderError {
                message: format!("Gemini API error (HTTP {status}): {response_body}"),
            });
        }

        Err(VectorCodeError::EmbedderError {
            message: "Gemini: max retries exceeded".to_string(),
        })
    }

    async fn embed_batch(&self, texts: &[&str]) -> EmbedderResult<Vec<Vec<f32>>> {
        let url = self.batch_url();
        let mut all_embeddings = Vec::with_capacity(texts.len());

        for chunk in texts.chunks(GEMINI_BATCH_SIZE) {
            let body = self.build_batch_request(chunk);

            for attempt in 0..=MAX_RETRIES {
                let response = self
                    .client
                    .post(&url)
                    .header("x-goog-api-key", self.api_key.expose_secret())
                    .json(&body)
                    .send()
                    .await
                    .map_err(|e| VectorCodeError::EmbedderError {
                        message: format!("Gemini batch HTTP request failed: {e}"),
                    })?;

                let status = response.status().as_u16();

                if response.status().is_success() {
                    let response_body =
                        response
                            .text()
                            .await
                            .map_err(|e| VectorCodeError::EmbedderError {
                                message: format!("Failed to read Gemini batch response body: {e}"),
                            })?;
                    let mut batch_vectors = Self::parse_batch_response(&response_body)?;
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
                            "Gemini batch API: max retries ({MAX_RETRIES}) exceeded (HTTP {status})"
                        ),
                    });
                }

                let response_body = response.text().await.unwrap_or_default();
                return Err(VectorCodeError::EmbedderError {
                    message: format!("Gemini batch API error (HTTP {status}): {response_body}"),
                });
            }
        }

        Ok(all_embeddings)
    }

    fn dimensions(&self) -> u32 {
        self.dimensions
    }

    fn provider_name(&self) -> &str {
        "gemini"
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
    fn gemini_new_fails_without_api_key() {
        let result = GeminiEmbedder::new(String::new(), "gemini-embedding-001".to_string(), 768);
        assert!(result.is_err(), "Empty API key should fail");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("GEMINI_API_KEY"),
            "Error should mention env var, got: {msg}"
        );
    }

    #[test]
    fn gemini_new_fails_with_invalid_dimensions() {
        let result = GeminiEmbedder::new(
            "test-key".to_string(),
            "gemini-embedding-001".to_string(),
            999,
        );
        assert!(result.is_err(), "Invalid dimensions should fail");
        let err = result.unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("999"),
            "Error should mention invalid dimensions, got: {msg}"
        );
        assert!(
            msg.contains("Matryoshka"),
            "Error should mention Matryoshka, got: {msg}"
        );
    }

    #[test]
    fn gemini_new_succeeds_with_valid_config() {
        let embedder = GeminiEmbedder::new(
            "test-key".to_string(),
            "gemini-embedding-001".to_string(),
            768,
        );
        assert!(embedder.is_ok(), "Valid config should succeed");
    }

    #[test]
    fn gemini_all_matryoshka_dimensions_accepted() {
        for &dims in &[256u32, 512, 768, 1024, 3072] {
            let result =
                GeminiEmbedder::new("key".to_string(), "gemini-embedding-001".to_string(), dims);
            assert!(result.is_ok(), "Dimension {dims} should be valid");
            assert_eq!(result.unwrap().dimensions(), dims);
        }
    }

    #[test]
    fn gemini_metadata_methods() {
        let embedder =
            GeminiEmbedder::new("key".to_string(), "gemini-embedding-001".to_string(), 768)
                .unwrap();
        assert_eq!(embedder.provider_name(), "gemini");
        assert_eq!(embedder.model_name(), "gemini-embedding-001");
        assert_eq!(embedder.dimensions(), 768);
        assert_eq!(embedder.max_tokens(), 2048);
    }

    #[test]
    fn gemini_embed_url_contains_model_and_key() {
        let embedder = GeminiEmbedder::new(
            "my-api-key".to_string(),
            "gemini-embedding-001".to_string(),
            768,
        )
        .unwrap();
        let url = embedder.embed_url();
        assert!(
            url.contains("gemini-embedding-001"),
            "URL should contain model name: {url}"
        );
        assert!(
            !url.contains("my-api-key"),
            "URL must NOT contain API key (C1 fix): {url}"
        );
        assert!(
            url.contains("embedContent"),
            "URL should contain embedContent endpoint: {url}"
        );
        assert!(
            url.starts_with("https://generativelanguage.googleapis.com"),
            "URL should use correct base: {url}"
        );
    }

    #[test]
    fn gemini_batch_url_contains_batch_endpoint() {
        let embedder =
            GeminiEmbedder::new("key".to_string(), "gemini-embedding-001".to_string(), 768)
                .unwrap();
        let url = embedder.batch_url();
        assert!(
            url.contains("batchEmbedContents"),
            "Batch URL should contain batchEmbedContents: {url}"
        );
        assert!(
            url.contains("gemini-embedding-001"),
            "Batch URL should contain model: {url}"
        );
    }

    #[test]
    fn gemini_embed_request_body_format() {
        let embedder =
            GeminiEmbedder::new("key".to_string(), "gemini-embedding-001".to_string(), 768)
                .unwrap();
        let body = embedder.build_embed_request("hello world");

        assert_eq!(body["content"]["parts"][0]["text"], "hello world");
        assert_eq!(body["outputDimensionality"], 768);
    }

    #[test]
    fn gemini_batch_request_body_format() {
        let embedder =
            GeminiEmbedder::new("key".to_string(), "gemini-embedding-001".to_string(), 512)
                .unwrap();
        let texts = vec!["chunk one", "chunk two", "chunk three"];
        let body = embedder.build_batch_request(&texts);

        let requests = body["requests"].as_array().unwrap();
        assert_eq!(requests.len(), 3, "Should have one request per text");
        assert_eq!(requests[0]["content"]["parts"][0]["text"], "chunk one");
        assert_eq!(requests[1]["content"]["parts"][0]["text"], "chunk two");
        assert_eq!(requests[2]["content"]["parts"][0]["text"], "chunk three");
        assert_eq!(
            requests[0]["outputDimensionality"], 512,
            "Each request should include dimensionality"
        );
        assert_eq!(
            requests[0]["model"], "models/gemini-embedding-001",
            "Each batch request must include model with models/ prefix"
        );
        assert_eq!(
            requests[1]["model"], "models/gemini-embedding-001",
            "All requests must carry the same model"
        );
    }

    #[test]
    fn gemini_parse_embed_response_success() {
        let json = r#"{"embedding": {"values": [0.1, 0.2, 0.3, -0.5]}}"#;
        let result = GeminiEmbedder::parse_embed_response(json).unwrap();
        assert_eq!(result.len(), 4, "Should parse 4 values");
        assert!((result[0] - 0.1).abs() < 1e-6);
        assert!((result[3] - (-0.5)).abs() < 1e-6);
    }

    #[test]
    fn gemini_parse_batch_response_success() {
        let json = r#"{
            "embeddings": [
                {"values": [0.1, 0.2]},
                {"values": [0.3, 0.4]}
            ]
        }"#;
        let result = GeminiEmbedder::parse_batch_response(json).unwrap();
        assert_eq!(result.len(), 2, "Should parse 2 embedding vectors");
        assert_eq!(result[0].len(), 2, "First vector should have 2 dims");
        assert!((result[1][0] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn gemini_parse_response_invalid_json() {
        let result = GeminiEmbedder::parse_embed_response("not json");
        assert!(result.is_err(), "Invalid JSON should fail");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("parse"), "Error should mention parsing: {msg}");
    }
}
