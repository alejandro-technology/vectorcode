/// Language registry for tree-sitter parsers.
use std::sync::OnceLock;

/// Supported programming languages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SupportedLanguage {
    TypeScript,
    Tsx,
    JavaScript,
    Jsx,
    Python,
    Rust,
    Go,
    Java,
    Unknown,
}

impl SupportedLanguage {
    /// Detect language from file extension.
    pub fn from_extension(ext: &str) -> Self {
        match ext.to_lowercase().as_str() {
            "ts" => SupportedLanguage::TypeScript,
            "tsx" => SupportedLanguage::Tsx,
            "js" => SupportedLanguage::JavaScript,
            "jsx" => SupportedLanguage::Jsx,
            "mjs" | "cjs" => SupportedLanguage::JavaScript,
            "py" => SupportedLanguage::Python,
            "rs" => SupportedLanguage::Rust,
            "go" => SupportedLanguage::Go,
            "java" => SupportedLanguage::Java,
            _ => SupportedLanguage::Unknown,
        }
    }

    /// Get the tree-sitter Language instance for this language.
    pub fn tree_sitter_language(&self) -> Option<tree_sitter::Language> {
        match self {
            SupportedLanguage::TypeScript => get_typescript_language(),
            SupportedLanguage::Tsx => get_tsx_language(),
            SupportedLanguage::JavaScript | SupportedLanguage::Jsx => get_javascript_language(),
            SupportedLanguage::Python => get_python_language(),
            SupportedLanguage::Rust => get_rust_language(),
            SupportedLanguage::Go => get_go_language(),
            SupportedLanguage::Java => get_java_language(),
            SupportedLanguage::Unknown => None,
        }
    }

    /// Get the language identifier string.
    pub fn as_str(&self) -> &'static str {
        match self {
            SupportedLanguage::TypeScript => "typescript",
            SupportedLanguage::Tsx => "tsx",
            SupportedLanguage::JavaScript => "javascript",
            SupportedLanguage::Jsx => "jsx",
            SupportedLanguage::Python => "python",
            SupportedLanguage::Rust => "rust",
            SupportedLanguage::Go => "go",
            SupportedLanguage::Java => "java",
            SupportedLanguage::Unknown => "unknown",
        }
    }
}

// Lazy initialization for tree-sitter languages
static TYPESCRIPT_LANGUAGE: OnceLock<tree_sitter::Language> = OnceLock::new();
static TSX_LANGUAGE: OnceLock<tree_sitter::Language> = OnceLock::new();
static JAVASCRIPT_LANGUAGE: OnceLock<tree_sitter::Language> = OnceLock::new();
static PYTHON_LANGUAGE: OnceLock<tree_sitter::Language> = OnceLock::new();
static RUST_LANGUAGE: OnceLock<tree_sitter::Language> = OnceLock::new();
static GO_LANGUAGE: OnceLock<tree_sitter::Language> = OnceLock::new();
static JAVA_LANGUAGE: OnceLock<tree_sitter::Language> = OnceLock::new();

fn get_typescript_language() -> Option<tree_sitter::Language> {
    Some(
        TYPESCRIPT_LANGUAGE
            .get_or_init(|| {
                let lang_fn = tree_sitter_typescript::LANGUAGE_TYPESCRIPT;
                lang_fn.into()
            })
            .clone(),
    )
}

fn get_tsx_language() -> Option<tree_sitter::Language> {
    Some(
        TSX_LANGUAGE
            .get_or_init(|| {
                let lang_fn = tree_sitter_typescript::LANGUAGE_TSX;
                lang_fn.into()
            })
            .clone(),
    )
}

fn get_javascript_language() -> Option<tree_sitter::Language> {
    Some(
        JAVASCRIPT_LANGUAGE
            .get_or_init(|| {
                let lang_fn = tree_sitter_javascript::LANGUAGE;
                lang_fn.into()
            })
            .clone(),
    )
}

fn get_python_language() -> Option<tree_sitter::Language> {
    Some(
        PYTHON_LANGUAGE
            .get_or_init(|| {
                let lang_fn = tree_sitter_python::LANGUAGE;
                lang_fn.into()
            })
            .clone(),
    )
}

fn get_rust_language() -> Option<tree_sitter::Language> {
    Some(
        RUST_LANGUAGE
            .get_or_init(|| {
                let lang_fn = tree_sitter_rust::LANGUAGE;
                lang_fn.into()
            })
            .clone(),
    )
}

fn get_go_language() -> Option<tree_sitter::Language> {
    Some(
        GO_LANGUAGE
            .get_or_init(|| {
                let lang_fn = tree_sitter_go::LANGUAGE;
                lang_fn.into()
            })
            .clone(),
    )
}

fn get_java_language() -> Option<tree_sitter::Language> {
    Some(
        JAVA_LANGUAGE
            .get_or_init(|| {
                let lang_fn = tree_sitter_java::LANGUAGE;
                lang_fn.into()
            })
            .clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_typescript_from_extension() {
        assert_eq!(
            SupportedLanguage::from_extension("ts"),
            SupportedLanguage::TypeScript
        );
    }

    #[test]
    fn detect_tsx_from_extension() {
        assert_eq!(
            SupportedLanguage::from_extension("tsx"),
            SupportedLanguage::Tsx
        );
    }

    #[test]
    fn detect_javascript_from_extension() {
        assert_eq!(
            SupportedLanguage::from_extension("js"),
            SupportedLanguage::JavaScript
        );
        assert_eq!(
            SupportedLanguage::from_extension("jsx"),
            SupportedLanguage::Jsx
        );
        assert_eq!(
            SupportedLanguage::from_extension("mjs"),
            SupportedLanguage::JavaScript
        );
        assert_eq!(
            SupportedLanguage::from_extension("cjs"),
            SupportedLanguage::JavaScript
        );
    }

    #[test]
    fn detect_python_from_extension() {
        assert_eq!(
            SupportedLanguage::from_extension("py"),
            SupportedLanguage::Python
        );
    }

    #[test]
    fn detect_rust_from_extension() {
        assert_eq!(
            SupportedLanguage::from_extension("rs"),
            SupportedLanguage::Rust
        );
    }

    #[test]
    fn detect_go_from_extension() {
        assert_eq!(
            SupportedLanguage::from_extension("go"),
            SupportedLanguage::Go
        );
    }

    #[test]
    fn detect_java_from_extension() {
        assert_eq!(
            SupportedLanguage::from_extension("java"),
            SupportedLanguage::Java
        );
    }

    #[test]
    fn detect_unknown_from_extension() {
        assert_eq!(
            SupportedLanguage::from_extension("txt"),
            SupportedLanguage::Unknown
        );
        assert_eq!(
            SupportedLanguage::from_extension("md"),
            SupportedLanguage::Unknown
        );
        assert_eq!(
            SupportedLanguage::from_extension("json"),
            SupportedLanguage::Unknown
        );
    }

    #[test]
    fn detect_case_insensitive() {
        assert_eq!(
            SupportedLanguage::from_extension("TS"),
            SupportedLanguage::TypeScript
        );
        assert_eq!(
            SupportedLanguage::from_extension("Py"),
            SupportedLanguage::Python
        );
        assert_eq!(
            SupportedLanguage::from_extension("RS"),
            SupportedLanguage::Rust
        );
    }

    #[test]
    fn language_as_str() {
        assert_eq!(SupportedLanguage::TypeScript.as_str(), "typescript");
        assert_eq!(SupportedLanguage::Python.as_str(), "python");
        assert_eq!(SupportedLanguage::Rust.as_str(), "rust");
        assert_eq!(SupportedLanguage::Unknown.as_str(), "unknown");
    }

    #[test]
    fn typescript_tree_sitter_language_loads() {
        let lang = SupportedLanguage::TypeScript.tree_sitter_language();
        assert!(lang.is_some(), "TypeScript language should load");
    }

    #[test]
    fn python_tree_sitter_language_loads() {
        let lang = SupportedLanguage::Python.tree_sitter_language();
        assert!(lang.is_some(), "Python language should load");
    }

    #[test]
    fn rust_tree_sitter_language_loads() {
        let lang = SupportedLanguage::Rust.tree_sitter_language();
        assert!(lang.is_some(), "Rust language should load");
    }

    #[test]
    fn unknown_language_returns_none() {
        let lang = SupportedLanguage::Unknown.tree_sitter_language();
        assert!(lang.is_none(), "Unknown language should return None");
    }
}
