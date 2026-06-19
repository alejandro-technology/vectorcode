//! Async search functionality with retry logic.

use std::time::Duration;

/// Search result with relevance score.
pub struct SearchResult {
    pub path: String,
    pub score: f32,
    pub snippet: String,
}

/// Execute a search query with exponential backoff retry.
pub async fn search_with_retry(
    query: &str,
    max_retries: u32,
) -> Result<Vec<SearchResult>, String> {
    let mut attempt = 0;
    let mut delay = Duration::from_millis(100);

    loop {
        match execute_search(query).await {
            Ok(results) => return Ok(results),
            Err(e) => {
                attempt += 1;
                if attempt >= max_retries {
                    return Err(format!("Search failed after {attempt} attempts: {e}"));
                }
                tokio::time::sleep(delay).await;
                delay *= 2; // Exponential backoff
            }
        }
    }
}

/// Internal search implementation.
async fn execute_search(query: &str) -> Result<Vec<SearchResult>, String> {
    // Simulated search
    Ok(vec![SearchResult {
        path: "src/lib.rs".to_string(),
        score: 0.95,
        snippet: format!("Match for: {query}"),
    }])
}
