//! IR metrics — pure functions for measuring search quality (REQ-BENCH-001).
//!
//! All functions are stateless and trivially unit-testable.
//! - `recall_at_k`: fraction of relevant docs found in top-k
//! - `ndcg_at_k`: normalized discounted cumulative gain (graded relevance 0-3)
//! - `mrr`: mean reciprocal rank

use std::collections::{HashMap, HashSet};

/// Recall@k — fraction of relevant documents found in the top-k results.
///
/// `predicted`: ordered list of file paths (ranked by score, highest first)
/// `relevant`: set of file paths that are relevant (grade >= 1)
/// `k`: cutoff position
///
/// Returns 0.0 if `relevant` is empty or `predicted` is empty.
pub fn recall_at_k(predicted: &[String], relevant: &HashSet<String>, k: usize) -> f64 {
    if relevant.is_empty() || predicted.is_empty() {
        return 0.0;
    }

    let top_k = &predicted[..predicted.len().min(k)];
    let found = top_k.iter().filter(|p| relevant.contains(*p)).count();

    found as f64 / relevant.len() as f64
}

/// nDCG@k — normalized discounted cumulative gain with graded relevance.
///
/// `predicted`: ordered list of file paths (ranked by score, highest first)
/// `grades`: map from file path to relevance grade (0-3, where 3 is most relevant)
/// `k`: cutoff position
///
/// Returns 0.0 if the ideal DCG is 0 (no relevant documents in the universe).
pub fn ndcg_at_k(predicted: &[String], grades: &HashMap<String, f64>, k: usize) -> f64 {
    let dcg = compute_dcg(predicted, grades, k);
    let ideal_dcg = compute_ideal_dcg(grades, k);

    if ideal_dcg == 0.0 {
        return 0.0;
    }

    dcg / ideal_dcg
}

/// Compute DCG (discounted cumulative gain) for a ranked list.
///
/// DCG = sum_{i=1}^{k} rel_i / log2(i + 1)
/// where i is 1-indexed position.
fn compute_dcg(predicted: &[String], grades: &HashMap<String, f64>, k: usize) -> f64 {
    let top_k = &predicted[..predicted.len().min(k)];
    top_k
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let rel = grades.get(path).copied().unwrap_or(0.0);
            let position = (i + 1) as f64; // 1-indexed
            rel / (position + 1.0).log2() // log2(i + 1) where i is 1-indexed
        })
        .sum()
}

/// Compute ideal DCG — best possible ranking of the top-k items.
///
/// Sorts all graded items by relevance (descending) and computes DCG on the top-k.
fn compute_ideal_dcg(grades: &HashMap<String, f64>, k: usize) -> f64 {
    let mut sorted_grades: Vec<f64> = grades.values().copied().collect();
    sorted_grades.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    sorted_grades
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, &rel)| {
            let position = (i + 1) as f64; // 1-indexed
            rel / (position + 1.0).log2() // log2(i + 1) where i is 1-indexed
        })
        .sum()
}

/// MRR (mean reciprocal rank) — average of 1/rank for the first relevant result.
///
/// `predicted`: ordered list of file paths (ranked by score, highest first)
/// `relevant`: set of file paths that are relevant (grade >= 1)
///
/// Returns 0.0 if no relevant document is found or if `predicted` is empty.
pub fn mrr(predicted: &[String], relevant: &HashSet<String>) -> f64 {
    if predicted.is_empty() || relevant.is_empty() {
        return 0.0;
    }

    for (i, path) in predicted.iter().enumerate() {
        if relevant.contains(path) {
            return 1.0 / (i + 1) as f64;
        }
    }

    0.0
}

/// Symbol recall@k — fraction of expected symbols found in top-k predicted symbols.
///
/// `predicted`: ordered list of symbol keys (e.g., "file.rs::symbol")
/// `expected`: set of expected symbol keys (grade >= 1)
/// `k`: cutoff position
///
/// Returns 0.0 if `expected` is empty or `predicted` is empty.
pub fn symbol_recall_at_k(predicted: &[String], expected: &HashSet<String>, k: usize) -> f64 {
    if expected.is_empty() || predicted.is_empty() {
        return 0.0;
    }

    let top_k = &predicted[..predicted.len().min(k)];
    let found = top_k.iter().filter(|p| expected.contains(*p)).count();

    found as f64 / expected.len() as f64
}

/// Symbol precision@k — fraction of top-k predicted symbols that are expected.
///
/// `predicted`: ordered list of symbol keys (e.g., "file.rs::symbol")
/// `expected`: set of expected symbol keys (grade >= 1)
/// `k`: cutoff position
///
/// Returns 0.0 if `predicted` is empty.
pub fn symbol_precision_at_k(predicted: &[String], expected: &HashSet<String>, k: usize) -> f64 {
    if predicted.is_empty() {
        return 0.0;
    }

    let top_k = &predicted[..predicted.len().min(k)];
    let found = top_k.iter().filter(|p| expected.contains(*p)).count();

    found as f64 / k as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    // ─── recall_at_k tests ─────────────────────────────────────────────

    #[test]
    fn test_recall_at_5_empty() {
        let predicted: Vec<String> = vec![];
        let relevant: HashSet<String> = ["a.rs".into(), "b.rs".into()].into_iter().collect();

        assert_eq!(recall_at_k(&predicted, &relevant, 5), 0.0);
    }

    #[test]
    fn test_recall_at_5_no_relevant() {
        let predicted = vec!["a.rs".to_string(), "b.rs".to_string()];
        let relevant: HashSet<String> = HashSet::new();

        assert_eq!(recall_at_k(&predicted, &relevant, 5), 0.0);
    }

    #[test]
    fn test_recall_at_5_perfect() {
        let predicted = vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()];
        let relevant: HashSet<String> = ["a.rs".into(), "b.rs".into(), "c.rs".into()]
            .into_iter()
            .collect();

        assert_eq!(recall_at_k(&predicted, &relevant, 5), 1.0);
    }

    #[test]
    fn test_recall_at_5_partial() {
        let predicted = vec![
            "a.rs".to_string(),
            "x.rs".to_string(),
            "b.rs".to_string(),
            "y.rs".to_string(),
            "z.rs".to_string(),
        ];
        let relevant: HashSet<String> = ["a.rs".into(), "b.rs".into(), "c.rs".into()]
            .into_iter()
            .collect();

        // Found 2 out of 3 relevant docs in top-5
        assert!((recall_at_k(&predicted, &relevant, 5) - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn test_recall_at_5_k_cutoff() {
        let predicted = vec![
            "a.rs".to_string(),
            "b.rs".to_string(),
            "c.rs".to_string(),
            "d.rs".to_string(),
            "e.rs".to_string(),
        ];
        let relevant: HashSet<String> = ["a.rs".into(), "e.rs".into()].into_iter().collect();

        // k=3: only "a.rs" is in top-3, so recall = 1/2 = 0.5
        assert_eq!(recall_at_k(&predicted, &relevant, 3), 0.5);
    }

    // ─── ndcg_at_k tests ───────────────────────────────────────────────

    #[test]
    fn test_ndcg_at_5_perfect() {
        let predicted = vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()];
        let mut grades = HashMap::new();
        grades.insert("a.rs".to_string(), 3.0);
        grades.insert("b.rs".to_string(), 2.0);
        grades.insert("c.rs".to_string(), 1.0);

        // Perfect ranking: DCG = ideal DCG, so nDCG = 1.0
        assert!((ndcg_at_k(&predicted, &grades, 5) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_ndcg_at_5_no_relevant() {
        let predicted = vec!["a.rs".to_string(), "b.rs".to_string()];
        let grades: HashMap<String, f64> = HashMap::new();

        assert_eq!(ndcg_at_k(&predicted, &grades, 5), 0.0);
    }

    #[test]
    fn test_ndcg_at_5_suboptimal() {
        // Predicted order: low relevance first, then high
        let predicted = vec![
            "a.rs".to_string(), // grade 1
            "b.rs".to_string(), // grade 3
        ];
        let mut grades = HashMap::new();
        grades.insert("a.rs".to_string(), 1.0);
        grades.insert("b.rs".to_string(), 3.0);

        // DCG = 1/log2(2) + 3/log2(3) = 1/1 + 3/1.585 = 1 + 1.893 = 2.893
        // Ideal DCG = 3/log2(2) + 1/log2(3) = 3/1 + 1/1.585 = 3 + 0.631 = 3.631
        // nDCG = 2.893 / 3.631 ≈ 0.797
        let ndcg = ndcg_at_k(&predicted, &grades, 5);
        assert!(
            ndcg > 0.7 && ndcg < 0.9,
            "nDCG should be ~0.797, got {ndcg}"
        );
    }

    #[test]
    fn test_ndcg_at_5_empty_predicted() {
        let predicted: Vec<String> = vec![];
        let mut grades = HashMap::new();
        grades.insert("a.rs".to_string(), 3.0);

        assert_eq!(ndcg_at_k(&predicted, &grades, 5), 0.0);
    }

    // ─── mrr tests ─────────────────────────────────────────────────────

    #[test]
    fn test_mrr_first() {
        let predicted = vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()];
        let relevant: HashSet<String> = ["a.rs".into()].into_iter().collect();

        // First relevant at position 1 → MRR = 1/1 = 1.0
        assert_eq!(mrr(&predicted, &relevant), 1.0);
    }

    #[test]
    fn test_mrr_second() {
        let predicted = vec!["a.rs".to_string(), "b.rs".to_string(), "c.rs".to_string()];
        let relevant: HashSet<String> = ["b.rs".into()].into_iter().collect();

        // First relevant at position 2 → MRR = 1/2 = 0.5
        assert_eq!(mrr(&predicted, &relevant), 0.5);
    }

    #[test]
    fn test_mrr_no_relevant() {
        let predicted = vec!["a.rs".to_string(), "b.rs".to_string()];
        let relevant: HashSet<String> = ["x.rs".into()].into_iter().collect();

        assert_eq!(mrr(&predicted, &relevant), 0.0);
    }

    #[test]
    fn test_mrr_empty() {
        let predicted: Vec<String> = vec![];
        let relevant: HashSet<String> = ["a.rs".into()].into_iter().collect();

        assert_eq!(mrr(&predicted, &relevant), 0.0);
    }

    #[test]
    fn test_mrr_empty_relevant() {
        let predicted = vec!["a.rs".to_string()];
        let relevant: HashSet<String> = HashSet::new();

        assert_eq!(mrr(&predicted, &relevant), 0.0);
    }

    // ─── symbol_recall_at_k tests ────────────────────────────────────────

    #[test]
    fn symbol_recall_perfect() {
        let predicted = vec![
            "a.rs::foo".to_string(),
            "b.rs::bar".to_string(),
            "c.rs::baz".to_string(),
        ];
        let expected: HashSet<String> =
            ["a.rs::foo".into(), "b.rs::bar".into(), "c.rs::baz".into()]
                .into_iter()
                .collect();

        assert_eq!(symbol_recall_at_k(&predicted, &expected, 5), 1.0);
    }

    #[test]
    fn symbol_recall_partial() {
        let predicted = vec![
            "a.rs::foo".to_string(),
            "x.rs::noise".to_string(),
            "b.rs::bar".to_string(),
        ];
        let expected: HashSet<String> =
            ["a.rs::foo".into(), "b.rs::bar".into(), "c.rs::baz".into()]
                .into_iter()
                .collect();

        // Found 2 out of 3 expected in top-3
        assert!((symbol_recall_at_k(&predicted, &expected, 3) - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn symbol_recall_empty() {
        let predicted: Vec<String> = vec![];
        let expected: HashSet<String> = ["a.rs::foo".into()].into_iter().collect();

        assert_eq!(symbol_recall_at_k(&predicted, &expected, 5), 0.0);
    }

    // ─── symbol_precision_at_k tests ─────────────────────────────────────

    #[test]
    fn symbol_precision_perfect() {
        let predicted = vec!["a.rs::foo".to_string(), "b.rs::bar".to_string()];
        let expected: HashSet<String> = ["a.rs::foo".into(), "b.rs::bar".into()]
            .into_iter()
            .collect();

        // Both in top-2 are expected
        assert_eq!(symbol_precision_at_k(&predicted, &expected, 2), 1.0);
    }

    #[test]
    fn symbol_precision_with_noise() {
        let predicted = vec![
            "a.rs::foo".to_string(),
            "x.rs::noise".to_string(),
            "b.rs::bar".to_string(),
        ];
        let expected: HashSet<String> = ["a.rs::foo".into(), "b.rs::bar".into()]
            .into_iter()
            .collect();

        // 2 out of 3 in top-3 are expected
        assert!((symbol_precision_at_k(&predicted, &expected, 3) - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn symbol_precision_empty() {
        let predicted: Vec<String> = vec![];
        let expected: HashSet<String> = ["a.rs::foo".into()].into_iter().collect();

        assert_eq!(symbol_precision_at_k(&predicted, &expected, 5), 0.0);
    }
}
