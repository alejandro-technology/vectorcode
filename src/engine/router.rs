//! Query classifier for routing structural queries to the graph retriever.
//!
//! Pure function that inspects the query string and determines whether it should
//! be routed to the graph retriever (for structural queries like "who calls X")
//! or to the existing hybrid/dense retriever (for semantic queries).

/// Result of classifying a query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoutingDecision {
    /// Route to hybrid/dense retriever (semantic query).
    Hybrid,
    /// Route to graph retriever with structured query.
    Graph(GraphQuery),
}

/// Structured graph query extracted from natural language.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphQuery {
    /// What kind of graph query.
    pub kind: GraphQueryKind,
    /// The target symbol.
    pub symbol: String,
    /// Optional file path for disambiguation (not extracted by heuristic).
    pub file_path: Option<String>,
}

/// Kind of graph query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphQueryKind {
    /// Find callers of a symbol (Call edges in).
    Callers,
    /// Find dependents of a symbol (Import/Extends/Reference edges in).
    Dependents,
    /// Find imports of a symbol (Import edges out).
    Imports,
}

/// Classify a query string to determine routing.
///
/// Uses regex patterns to detect structural query phrasings:
/// - "who calls X", "what calls X", "callers of X" → Callers
/// - "who depends on X", "dependents of X" → Dependents
/// - "imports of X", "what does X import" → Imports
/// - Everything else → Hybrid
///
/// This is a pure function with no side effects.
pub fn classify_query(query: &str) -> RoutingDecision {
    let lower = query.to_lowercase();
    let lower = lower.trim();

    // Callers patterns
    if let Some(caps) = regex::Regex::new(r"^(?:who|what)\s+(?:calls?|invokes?)\s+(\w+)")
        .ok()
        .and_then(|re| re.captures(lower))
    {
        if let Some(m) = caps.get(1) {
            return RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Callers,
                symbol: m.as_str().to_string(),
                file_path: None,
            });
        }
    }

    if let Some(caps) = regex::Regex::new(r"^callers?\s+of\s+(\w+)")
        .ok()
        .and_then(|re| re.captures(lower))
    {
        if let Some(m) = caps.get(1) {
            return RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Callers,
                symbol: m.as_str().to_string(),
                file_path: None,
            });
        }
    }

    // Dependents patterns
    if let Some(caps) = regex::Regex::new(r"^(?:who|what)\s+depends?\s+on\s+(\w+)")
        .ok()
        .and_then(|re| re.captures(lower))
    {
        if let Some(m) = caps.get(1) {
            return RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Dependents,
                symbol: m.as_str().to_string(),
                file_path: None,
            });
        }
    }

    if let Some(caps) = regex::Regex::new(r"^dependents?\s+of\s+(\w+)")
        .ok()
        .and_then(|re| re.captures(lower))
    {
        if let Some(m) = caps.get(1) {
            return RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Dependents,
                symbol: m.as_str().to_string(),
                file_path: None,
            });
        }
    }

    // Imports patterns
    if let Some(caps) = regex::Regex::new(r"^(?:what\s+)?imports?\s+of\s+(\w+)")
        .ok()
        .and_then(|re| re.captures(lower))
    {
        if let Some(m) = caps.get(1) {
            return RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Imports,
                symbol: m.as_str().to_string(),
                file_path: None,
            });
        }
    }

    if let Some(caps) = regex::Regex::new(r"^what\s+does\s+(\w+)\s+import$")
        .ok()
        .and_then(|re| re.captures(lower))
    {
        if let Some(m) = caps.get(1) {
            return RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Imports,
                symbol: m.as_str().to_string(),
                file_path: None,
            });
        }
    }

    // Default: hybrid
    RoutingDecision::Hybrid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_who_calls_returns_callers() {
        let result = classify_query("who calls authenticate");
        assert_eq!(
            result,
            RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Callers,
                symbol: "authenticate".to_string(),
                file_path: None,
            })
        );
    }

    #[test]
    fn classify_what_calls_returns_callers() {
        let result = classify_query("what calls search");
        assert_eq!(
            result,
            RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Callers,
                symbol: "search".to_string(),
                file_path: None,
            })
        );
    }

    #[test]
    fn classify_callers_of_returns_callers() {
        let result = classify_query("callers of main");
        assert_eq!(
            result,
            RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Callers,
                symbol: "main".to_string(),
                file_path: None,
            })
        );
    }

    #[test]
    fn classify_what_depends_on_returns_dependents() {
        let result = classify_query("what depends on Base");
        assert_eq!(
            result,
            RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Dependents,
                symbol: "base".to_string(),
                file_path: None,
            })
        );
    }

    #[test]
    fn classify_dependents_of_returns_dependents() {
        let result = classify_query("dependents of Foo");
        assert_eq!(
            result,
            RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Dependents,
                symbol: "foo".to_string(),
                file_path: None,
            })
        );
    }

    #[test]
    fn classify_imports_of_returns_imports() {
        let result = classify_query("imports of my_module");
        assert_eq!(
            result,
            RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Imports,
                symbol: "my_module".to_string(),
                file_path: None,
            })
        );
    }

    #[test]
    fn classify_what_does_x_import_returns_imports() {
        let result = classify_query("what does my_module import");
        assert_eq!(
            result,
            RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Imports,
                symbol: "my_module".to_string(),
                file_path: None,
            })
        );
    }

    #[test]
    fn classify_semantic_query_returns_hybrid() {
        let result = classify_query("how does authentication work");
        assert_eq!(result, RoutingDecision::Hybrid);
    }

    #[test]
    fn classify_empty_query_returns_hybrid() {
        let result = classify_query("");
        assert_eq!(result, RoutingDecision::Hybrid);
    }

    #[test]
    fn classify_case_insensitive() {
        let result = classify_query("WHO CALLS Authenticate");
        assert_eq!(
            result,
            RoutingDecision::Graph(GraphQuery {
                kind: GraphQueryKind::Callers,
                symbol: "authenticate".to_string(),
                file_path: None,
            })
        );
    }

    #[test]
    fn classify_extracts_symbol() {
        let result = classify_query("who calls search_with_retry");
        match result {
            RoutingDecision::Graph(q) => {
                assert_eq!(q.symbol, "search_with_retry");
            }
            _ => panic!("Expected Graph routing"),
        }
    }
}
