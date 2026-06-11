//! Ollama embedding provider.
//!
//! Uses a local Ollama instance for text embeddings.
//! Default model: nomic-embed-text (768 dimensions, 8192 max tokens).
//! Spec §7.2: Ollama

use crate::embedder::{Embedder, EmbedderResult};
use crate::error::VectorCodeError;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// Default batch size for Ollama requests.
const OLLAMA_BATCH_SIZE: usize = 100;

/// Ollama embedding provider.
///
/// Sends text to a local Ollama instance and returns vector embeddings.
/// No API key required — Ollama runs locally.
pub struct OllamaEmbedder {
    base_url: String,
    model: String,
    client: reqwest::Client,
}

impl OllamaEmbedder {
    /// Default model identifier.
    pub const DEFAULT_MODEL: &'static str = "nomic-embed-text";
    /// Default base URL for local Ollama.
    pub const DEFAULT_URL: &'static str = "http://localhost:11434";
    /// Output dimensions for the default model.
    pub const DEFAULT_DIMENSIONS: u32 = 768;
    /// Maximum input token length.
    pub const MAX_TOKENS: u32 = 8192;

    /// Create a new OllamaEmbedder with default settings.
    pub fn new() -> EmbedderResult<Self> {
        Self::with_client(
            Self::DEFAULT_URL.to_string(),
            Self::DEFAULT_MODEL.to_string(),
            reqwest::Client::new(),
        )
    }

    /// Create with custom URL and model.
    pub fn with_config(url: String, model: String) -> EmbedderResult<Self> {
        Self::with_client(url, model, reqwest::Client::new())
    }

    /// Create with a custom reqwest::Client (useful for testing).
    pub fn with_client(
        url: String,
        model: String,
        client: reqwest::Client,
    ) -> EmbedderResult<Self> {
        let base_url = url.trim_end_matches('/').to_string();
        if base_url.is_empty() {
            return Err(VectorCodeError::EmbedderError {
                message: "Ollama URL cannot be empty".to_string(),
            });
        }
        if model.is_empty() {
            return Err(VectorCodeError::EmbedderError {
                message: "Ollama model name cannot be empty".to_string(),
            });
        }
        Ok(Self {
            base_url,
            model,
            client,
        })
    }

    /// Build the embed endpoint URL.
    fn embed_url(&self) -> String {
        format!("{}/api/embed", self.base_url)
    }

    /// Build request body for embed (works for both single and batch).
    fn build_request(&self, texts: &[&str]) -> OllamaRequest {
        OllamaRequest {
            model: self.model.clone(),
            input: texts.iter().map(|t| t.to_string()).collect(),
        }
    }

    /// Parse embed response JSON into vectors.
    fn parse_response(body: &str) -> EmbedderResult<Vec<Vec<f32>>> {
        let response: OllamaResponse =
            serde_json::from_str(body).map_err(|e| VectorCodeError::EmbedderError {
                message: format!("Failed to parse Ollama response: {e}"),
            })?;
        Ok(response.embeddings)
    }
}

impl Default for OllamaEmbedder {
    fn default() -> Self {
        Self::new().expect("Default Ollama config should always be valid")
    }
}

/// Ollama embed request body.
#[derive(Serialize)]
struct OllamaRequest {
    model: String,
    input: Vec<String>,
}

/// Ollama embed response body.
#[derive(Deserialize)]
struct OllamaResponse {
    embeddings: Vec<Vec<f32>>,
}

#[async_trait]
impl Embedder for OllamaEmbedder {
    async fn embed(&self, text: &str) -> EmbedderResult<Vec<f32>> {
        let url = self.embed_url();
        let body = self.build_request(&[text]);

        let response = self
            .client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| VectorCodeError::EmbedderError {
                message: format!("Ollama HTTP request failed: {e}"),
            })?;

        let status = response.status().as_u16();
        if !response.status().is_success() {
            let response_body = response.text().await.unwrap_or_default();
            return Err(VectorCodeError::EmbedderError {
                message: format!("Ollama API error (HTTP {status}): {response_body}"),
            });
        }

        let response_body = response
            .text()
            .await
            .map_err(|e| VectorCodeError::EmbedderError {
                message: format!("Failed to read Ollama response body: {e}"),
            })?;

        let vectors = Self::parse_response(&response_body)?;
        vectors.into_iter().next().ok_or_else(|| {
            {
                VectorCodeError::EmbedderError {
                    message: "Ollama returned empty embeddings array".to_string(),
                }
            }
            .into()
        })
    }

    async fn embed_batch(&self, texts: &[&str]) -> EmbedderResult<Vec<Vec<f32>>> {
        let url = self.embed_url();
        let mut all_embeddings = Vec::with_capacity(texts.len());

        for chunk in texts.chunks(OLLAMA_BATCH_SIZE) {
            let body = self.build_request(chunk);

            let response = self
                .client
                .post(&url)
                .json(&body)
                .send()
                .await
                .map_err(|e| VectorCodeError::EmbedderError {
                    message: format!("Ollama batch HTTP request failed: {e}"),
                })?;

            let status = response.status().as_u16();
            if !response.status().is_success() {
                let response_body = response.text().await.unwrap_or_default();
                return Err(VectorCodeError::EmbedderError {
                    message: format!("Ollama batch API error (HTTP {status}): {response_body}"),
                });
            }

            let response_body =
                response
                    .text()
                    .await
                    .map_err(|e| VectorCodeError::EmbedderError {
                        message: format!("Failed to read Ollama batch response body: {e}"),
                    })?;

            let mut batch_vectors = Self::parse_response(&response_body)?;
            all_embeddings.append(&mut batch_vectors);
        }

        Ok(all_embeddings)
    }

    fn dimensions(&self) -> u32 {
        Self::DEFAULT_DIMENSIONS
    }

    fn provider_name(&self) -> &str {
        "ollama"
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
    fn ollama_new_with_default_config() {
        let embedder = OllamaEmbedder::new().unwrap();
        assert_eq!(embedder.base_url, "http://localhost:11434");
        assert_eq!(embedder.model, "nomic-embed-text");
    }

    #[test]
    fn ollama_new_with_custom_config() {
        let embedder = OllamaEmbedder::with_config(
            "http://custom:11434".to_string(),
            "mxbai-embed-large".to_string(),
        )
        .unwrap();
        assert_eq!(embedder.base_url, "http://custom:11434");
        assert_eq!(embedder.model, "mxbai-embed-large");
    }

    #[test]
    fn ollama_new_strips_trailing_slash() {
        let embedder =
            OllamaEmbedder::with_config("http://localhost:11434/".to_string(), "model".to_string())
                .unwrap();
        assert_eq!(embedder.base_url, "http://localhost:11434");
    }

    #[test]
    fn ollama_new_fails_with_empty_url() {
        let result = OllamaEmbedder::with_client(
            "".to_string(),
            "model".to_string(),
            reqwest::Client::new(),
        );
        assert!(result.is_err(), "Empty URL should fail");
    }

    #[test]
    fn ollama_new_fails_with_empty_model() {
        let result = OllamaEmbedder::with_client(
            "http://localhost:11434".to_string(),
            "".to_string(),
            reqwest::Client::new(),
        );
        assert!(result.is_err(), "Empty model should fail");
    }

    #[test]
    fn ollama_metadata_methods() {
        let embedder = OllamaEmbedder::new().unwrap();
        assert_eq!(embedder.provider_name(), "ollama");
        assert_eq!(embedder.model_name(), "nomic-embed-text");
        assert_eq!(embedder.dimensions(), 768);
        assert_eq!(embedder.max_tokens(), 8192);
    }

    #[test]
    fn ollama_metadata_custom_model() {
        let embedder = OllamaEmbedder::with_config(
            "http://localhost:11434".to_string(),
            "mxbai-embed-large".to_string(),
        )
        .unwrap();
        assert_eq!(embedder.model_name(), "mxbai-embed-large");
    }

    #[test]
    fn ollama_embed_url_construction() {
        let embedder = OllamaEmbedder::new().unwrap();
        let url = embedder.embed_url();
        assert_eq!(url, "http://localhost:11434/api/embed");
    }

    #[test]
    fn ollama_embed_url_custom_base() {
        let embedder =
            OllamaEmbedder::with_config("http://myhost:8080".to_string(), "model".to_string())
                .unwrap();
        let url = embedder.embed_url();
        assert_eq!(url, "http://myhost:8080/api/embed");
    }

    #[test]
    fn ollama_request_body_format() {
        let embedder = OllamaEmbedder::new().unwrap();
        let body = embedder.build_request(&["hello world"]);
        assert_eq!(body.model, "nomic-embed-text");
        assert_eq!(body.input, vec!["hello world"]);
    }

    #[test]
    fn ollama_batch_request_body_multiple_inputs() {
        let embedder = OllamaEmbedder::new().unwrap();
        let texts = vec!["chunk one", "chunk two", "chunk three"];
        let body = embedder.build_request(&texts);
        assert_eq!(body.input.len(), 3, "Should include all inputs");
        assert_eq!(body.input[0], "chunk one");
        assert_eq!(body.input[2], "chunk three");
    }

    #[test]
    fn ollama_parse_response_success() {
        let json = r#"{
            "model": "nomic-embed-text",
            "embeddings": [[0.1, 0.2, 0.3], [0.4, 0.5, 0.6]]
        }"#;
        let result = OllamaEmbedder::parse_response(json).unwrap();
        assert_eq!(result.len(), 2, "Should parse 2 embedding vectors");
        assert_eq!(result[0].len(), 3, "First vector should have 3 dims");
        assert!((result[0][0] - 0.1).abs() < 1e-6);
        assert!((result[1][2] - 0.6).abs() < 1e-6);
    }

    #[test]
    fn ollama_parse_response_invalid_json() {
        let result = OllamaEmbedder::parse_response("not valid json");
        assert!(result.is_err(), "Invalid JSON should fail");
        let msg = result.unwrap_err().to_string();
        assert!(msg.contains("parse"), "Error should mention parsing: {msg}");
    }

    #[test]
    fn ollama_constants() {
        assert_eq!(OllamaEmbedder::DEFAULT_MODEL, "nomic-embed-text");
        assert_eq!(OllamaEmbedder::DEFAULT_URL, "http://localhost:11434");
        assert_eq!(OllamaEmbedder::DEFAULT_DIMENSIONS, 768);
        assert_eq!(OllamaEmbedder::MAX_TOKENS, 8192);
    }
}
