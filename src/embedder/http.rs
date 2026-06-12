//! Shared HTTP helpers for embedding providers.
//!
//! Provides HTTP client construction, exponential backoff calculation,
//! retry status checks, and jitter utilities used by Gemini, Ollama, and OpenAI providers.

use std::time::Duration;

/// Default HTTP timeout for embedding provider requests.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Default connect timeout.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

pub fn build_http_client() -> Result<reqwest::Client, crate::error::VectorCodeError> {
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .map_err(|e| crate::error::VectorCodeError::EmbedderError {
            message: format!("Failed to build HTTP client: {}", e),
        })
}

/// Maximum number of retry attempts for transient failures.
pub const MAX_RETRIES: u32 = 5;

/// Base delay for exponential backoff in milliseconds.
pub const BASE_BACKOFF_MS: u64 = 1000;

/// Maximum delay cap in milliseconds.
pub const MAX_BACKOFF_MS: u64 = 60_000;

/// Calculate exponential backoff with full jitter.
///
/// Uses the "full jitter" strategy: `sleep = random(0, min(max, base * 2^attempt))`.
/// The `random_factor` parameter should be in [0.0, 1.0] — callers generate
/// this from their preferred RNG source.
///
/// # Arguments
/// * `attempt` - Zero-based retry attempt number
/// * `base_ms` - Base delay in milliseconds
/// * `max_ms` - Maximum delay cap in milliseconds
/// * `random_factor` - Random value in [0.0, 1.0] for jitter
pub fn calculate_backoff(attempt: u32, base_ms: u64, max_ms: u64, random_factor: f64) -> Duration {
    let random_factor = random_factor.clamp(0.0, 1.0);
    let shift = attempt.min(20); // Prevent overflow on very high attempts
    let exponential = base_ms.saturating_mul(1u64 << shift);
    let capped = exponential.min(max_ms) as f64;
    // Full jitter: uniform random between 0 and capped
    let jittered = capped * random_factor;
    Duration::from_millis(jittered as u64)
}

/// Check if an HTTP status code should trigger a retry.
///
/// Retries on: 429 (rate limit), 500 (internal server error), 503 (service unavailable).
pub fn should_retry(status: u16) -> bool {
    matches!(status, 429 | 500 | 503)
}

/// Generate a jitter factor in [0.0, 1.0) from system time nanoseconds.
///
/// This is a cheap source of randomness suitable for backoff jitter.
/// Not cryptographically secure — do not use for security purposes.
pub fn jitter_factor() -> f64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .subsec_nanos();
    (nanos % 1000) as f64 / 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_first_attempt_zero_factor_returns_zero() {
        let d = calculate_backoff(0, 1000, 30_000, 0.0);
        assert_eq!(d.as_millis(), 0, "Zero factor should give zero delay");
    }

    #[test]
    fn backoff_first_attempt_full_factor_returns_base() {
        let d = calculate_backoff(0, 1000, 30_000, 1.0);
        assert_eq!(
            d.as_millis(),
            1000,
            "Factor 1.0 at attempt 0 should give base_ms"
        );
    }

    #[test]
    fn backoff_grows_exponentially() {
        let d1 = calculate_backoff(1, 1000, 60_000, 1.0);
        let d2 = calculate_backoff(2, 1000, 60_000, 1.0);
        let d3 = calculate_backoff(3, 1000, 60_000, 1.0);
        assert_eq!(d1.as_millis(), 2000, "Attempt 1 should be 2x base");
        assert_eq!(d2.as_millis(), 4000, "Attempt 2 should be 4x base");
        assert_eq!(d3.as_millis(), 8000, "Attempt 3 should be 8x base");
    }

    #[test]
    fn backoff_respects_max_cap() {
        let d = calculate_backoff(100, 1000, 30_000, 1.0);
        assert_eq!(d.as_millis(), 30_000, "Should not exceed max_ms");
    }

    #[test]
    fn backoff_jitter_half_factor_gives_half_value() {
        let d = calculate_backoff(0, 1000, 30_000, 0.5);
        assert_eq!(
            d.as_millis(),
            500,
            "0.5 factor at attempt 0 should give 500ms"
        );
    }

    #[test]
    fn backoff_clamps_factor_out_of_range() {
        let d_neg = calculate_backoff(0, 1000, 30_000, -0.5);
        assert_eq!(d_neg.as_millis(), 0, "Negative factor should clamp to 0");
        let d_over = calculate_backoff(0, 1000, 30_000, 1.5);
        assert_eq!(d_over.as_millis(), 1000, "Factor > 1.0 should clamp to 1.0");
    }

    #[test]
    fn should_retry_matches_rate_limit() {
        assert!(should_retry(429), "429 should retry");
    }

    #[test]
    fn should_retry_matches_server_errors() {
        assert!(should_retry(500), "500 should retry");
        assert!(should_retry(503), "503 should retry");
    }

    #[test]
    fn should_retry_rejects_non_retryable() {
        assert!(!should_retry(200), "200 should not retry");
        assert!(!should_retry(401), "401 should not retry");
        assert!(!should_retry(403), "403 should not retry");
        assert!(!should_retry(404), "404 should not retry");
    }

    #[test]
    fn jitter_factor_returns_value_in_range() {
        let factor = jitter_factor();
        assert!(
            (0.0..1.0).contains(&factor),
            "jitter_factor should be in [0.0, 1.0), got {factor}"
        );
    }

    #[test]
    fn constants_have_expected_values() {
        assert_eq!(MAX_RETRIES, 5);
        assert_eq!(BASE_BACKOFF_MS, 1000);
        assert_eq!(MAX_BACKOFF_MS, 60_000);
    }
}
