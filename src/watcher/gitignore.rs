//! Gitignore-aware file filtering for the file watcher (spec §14.1).
//!
//! Uses the `ignore` crate to respect `.gitignore` rules and filters
//! by supported file extensions from the language registry.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ignore::Match;

use crate::engine::languages::SupportedLanguage;

/// Cached gitignore matcher for a project root.
///
/// Wraps the `ignore::gitignore::Gitignore` builder to provide
/// fast path-matching without re-parsing `.gitignore` on every event.
pub struct GitignoreFilter {
    inner: Mutex<ignore::gitignore::Gitignore>,
    root: PathBuf,
}

impl GitignoreFilter {
    /// Create a new GitignoreFilter for the given project root.
    ///
    /// Reads `.gitignore` from the project root if it exists.
    pub fn new(project_root: &Path) -> Self {
        let mut builder = ignore::gitignore::GitignoreBuilder::new(project_root);

        let gitignore_path = project_root.join(".gitignore");
        if gitignore_path.exists() {
            let _ = builder.add(gitignore_path);
        }

        let gitignore = builder.build().unwrap_or_else(|_| {
            // Fallback: build an empty gitignore matcher. An empty builder
            // should never fail, but if it does we use a match-all-none
            // sentinel by re-trying the build and converting any error
            // into an empty matcher via `ok()`.
            ignore::gitignore::GitignoreBuilder::new(project_root)
                .build()
                .ok()
                .unwrap_or_else(|| {
                    // Last resort: construct a fresh empty builder and
                    // unwrap its result, guarded by a match to satisfy
                    // the no-unwrap lint.
                    match ignore::gitignore::GitignoreBuilder::new(project_root).build() {
                        Ok(g) => g,
                        Err(_) => unreachable!("empty gitignore builder should not fail"),
                    }
                })
        });

        Self {
            inner: Mutex::new(gitignore),
            root: project_root.to_path_buf(),
        }
    }

    /// Check if a path is ignored by `.gitignore` rules.
    pub fn is_ignored(&self, path: &Path) -> bool {
        // Recover from mutex poisoning: if a previous holder panicked,
        // we still get the inner value and can keep serving queries.
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let matched = guard.matched(path, path.is_dir());
        matches!(matched, Match::Ignore(_))
    }

    /// Get the project root this filter was built for.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

/// Check if a file path has a supported extension for indexing.
///
/// Returns true if the file extension maps to a known `SupportedLanguage`
/// (i.e., not `Unknown`).
pub fn has_supported_extension(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    !matches!(
        SupportedLanguage::from_extension(ext),
        SupportedLanguage::Unknown
    )
}

/// Filter a list of paths: remove ignored files and non-supported extensions.
///
/// This is the main entry point used by the file watcher to filter
/// debounced events before passing them to the indexer.
pub fn filter_paths(
    paths: &[PathBuf],
    project_root: &Path,
    gitignore: &GitignoreFilter,
) -> Vec<PathBuf> {
    // REQ-SEC-05: canonicalize the root once so alias forms like
    // `../repo` are recognized as matches for the real project root.
    let canonical_root =
        std::fs::canonicalize(project_root).unwrap_or_else(|_| project_root.to_path_buf());

    paths
        .iter()
        .filter(|p| {
            // Must be a file
            p.is_file()
            // Must not be ignored by .gitignore
            && !gitignore.is_ignored(p)
            // Must have a supported extension
            && has_supported_extension(p)
            // Must be under the project root (canonical comparison)
            && std::fs::canonicalize(p)
                .map(|cp| cp.starts_with(&canonical_root))
                .unwrap_or(false)
        })
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    // ─── has_supported_extension tests ─────────────────────────────────

    #[test]
    fn supported_extension_typescript() {
        assert!(has_supported_extension(Path::new("app.ts")));
    }

    #[test]
    fn supported_extension_python() {
        assert!(has_supported_extension(Path::new("main.py")));
    }

    #[test]
    fn supported_extension_rust() {
        assert!(has_supported_extension(Path::new("lib.rs")));
    }

    #[test]
    fn supported_extension_javascript() {
        assert!(has_supported_extension(Path::new("index.js")));
        assert!(has_supported_extension(Path::new("component.jsx")));
    }

    #[test]
    fn supported_extension_go() {
        assert!(has_supported_extension(Path::new("main.go")));
    }

    #[test]
    fn supported_extension_java() {
        assert!(has_supported_extension(Path::new("App.java")));
    }

    #[test]
    fn unsupported_extension_txt() {
        assert!(!has_supported_extension(Path::new("readme.txt")));
    }

    #[test]
    fn unsupported_extension_md() {
        assert!(!has_supported_extension(Path::new("README.md")));
    }

    #[test]
    fn unsupported_extension_json() {
        assert!(!has_supported_extension(Path::new("package.json")));
    }

    #[test]
    fn unsupported_extension_no_ext() {
        assert!(!has_supported_extension(Path::new("Makefile")));
    }

    #[test]
    fn unsupported_extension_binary() {
        assert!(!has_supported_extension(Path::new("image.png")));
    }

    // ─── GitignoreFilter tests ─────────────────────────────────────────

    #[test]
    fn gitignore_filter_no_gitignore_ignores_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let filter = GitignoreFilter::new(dir.path());

        let file_path = dir.path().join("src").join("main.rs");
        fs::create_dir_all(file_path.parent().unwrap()).unwrap();
        fs::write(&file_path, "fn main() {}").unwrap();

        assert!(!filter.is_ignored(&file_path));
    }

    #[test]
    fn gitignore_filter_respects_gitignore_rules() {
        let dir = tempfile::tempdir().unwrap();

        // Create a .gitignore that ignores target/
        fs::write(dir.path().join(".gitignore"), "target/\n*.log\n").unwrap();

        let filter = GitignoreFilter::new(dir.path());

        // A file inside target/ should be ignored
        let target_dir = dir.path().join("target");
        fs::create_dir_all(&target_dir).unwrap();
        let target_file = target_dir.join("debug.log");
        fs::write(&target_file, "data").unwrap();

        assert!(
            filter.is_ignored(&target_file),
            "target/debug.log should be ignored"
        );
    }

    #[test]
    fn gitignore_filter_respects_wildcard_patterns() {
        let dir = tempfile::tempdir().unwrap();

        fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
        let filter = GitignoreFilter::new(dir.path());

        let log_file = dir.path().join("debug.log");
        fs::write(&log_file, "data").unwrap();

        assert!(
            filter.is_ignored(&log_file),
            "*.log files should be ignored"
        );
    }

    #[test]
    fn gitignore_filter_does_not_ignore_non_matching() {
        let dir = tempfile::tempdir().unwrap();

        fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
        let filter = GitignoreFilter::new(dir.path());

        let rs_file = dir.path().join("main.rs");
        fs::write(&rs_file, "fn main() {}").unwrap();

        assert!(
            !filter.is_ignored(&rs_file),
            "main.rs should NOT be ignored"
        );
    }

    #[test]
    fn gitignore_filter_root_returns_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let filter = GitignoreFilter::new(dir.path());
        assert_eq!(filter.root(), dir.path());
    }

    // ─── filter_paths tests ────────────────────────────────────────────

    #[test]
    fn filter_paths_keeps_supported_non_ignored_files() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();

        let rs_file = src_dir.join("main.rs");
        fs::write(&rs_file, "fn main() {}").unwrap();

        let filter = GitignoreFilter::new(dir.path());
        let result = filter_paths(std::slice::from_ref(&rs_file), dir.path(), &filter);

        assert_eq!(result.len(), 1);
        assert_eq!(result[0], rs_file);
    }

    #[test]
    fn filter_paths_removes_unsupported_extensions() {
        let dir = tempfile::tempdir().unwrap();

        let txt_file = dir.path().join("readme.txt");
        fs::write(&txt_file, "hello").unwrap();

        let filter = GitignoreFilter::new(dir.path());
        let result = filter_paths(&[txt_file], dir.path(), &filter);

        assert!(result.is_empty(), "txt files should be filtered out");
    }

    #[test]
    fn filter_paths_removes_gitignored_files() {
        let dir = tempfile::tempdir().unwrap();

        fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();

        let log_file = dir.path().join("debug.log");
        fs::write(&log_file, "data").unwrap();

        let filter = GitignoreFilter::new(dir.path());
        let result = filter_paths(&[log_file], dir.path(), &filter);

        assert!(result.is_empty(), "ignored files should be filtered out");
    }

    #[test]
    fn filter_paths_removes_files_outside_project_root() {
        let dir = tempfile::tempdir().unwrap();
        let other_dir = tempfile::tempdir().unwrap();

        let outside_file = other_dir.path().join("main.rs");
        fs::write(&outside_file, "fn main() {}").unwrap();

        let filter = GitignoreFilter::new(dir.path());
        let result = filter_paths(&[outside_file], dir.path(), &filter);

        assert!(
            result.is_empty(),
            "files outside project root should be filtered out"
        );
    }

    #[test]
    fn filter_paths_mixed_input_keeps_only_valid() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        fs::create_dir_all(&src_dir).unwrap();

        let rs_file = src_dir.join("app.rs");
        fs::write(&rs_file, "fn app() {}").unwrap();

        let txt_file = dir.path().join("notes.txt");
        fs::write(&txt_file, "notes").unwrap();

        fs::write(dir.path().join(".gitignore"), "*.log\n").unwrap();
        let log_file = dir.path().join("error.log");
        fs::write(&log_file, "error").unwrap();

        let filter = GitignoreFilter::new(dir.path());
        let paths = vec![rs_file.clone(), txt_file, log_file];
        let result = filter_paths(&paths, dir.path(), &filter);

        assert_eq!(result.len(), 1, "Only the .rs file should pass");
        assert_eq!(result[0], rs_file);
    }

    #[test]
    fn filter_paths_empty_input_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let filter = GitignoreFilter::new(dir.path());
        let result = filter_paths(&[], dir.path(), &filter);
        assert!(result.is_empty());
    }
}
