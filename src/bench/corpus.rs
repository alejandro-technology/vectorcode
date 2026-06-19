//! Corpus abstraction — hexagonal port for benchmark file sources.
//!
//! `Corpus` trait: prepare files in a destination directory.
//! `LocalCorpus`: copy from local test fixtures (no network).
//! `GitCorpus`: clone from git URL with optional sparse checkout.

use std::path::{Path, PathBuf};

use anyhow::Result;
use async_trait::async_trait;

/// Hexagonal port — corpus provides files for benchmarking.
#[async_trait]
pub trait Corpus: Send + Sync {
    /// Corpus name (e.g., "mini", "vscode").
    fn name(&self) -> &str;

    /// Prepare corpus files in the destination directory.
    ///
    /// Returns the list of file paths (relative to dest) that should be indexed.
    async fn prepare(&self, dest: &Path) -> Result<Vec<PathBuf>>;
}

/// Local corpus — copies files from a local directory (test fixtures).
pub struct LocalCorpus {
    /// Source directory containing test fixtures.
    src: PathBuf,

    /// File extensions to include (e.g., [".rs", ".ts", ".py"]).
    file_extensions: Vec<String>,

    /// Corpus name.
    name: String,
}

impl LocalCorpus {
    /// Create a new LocalCorpus.
    pub fn new(name: String, src: PathBuf, file_extensions: Vec<String>) -> Self {
        Self {
            src,
            file_extensions,
            name,
        }
    }

    /// Discover files matching the extension filter.
    fn discover_files(&self) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();

        if !self.src.exists() {
            anyhow::bail!("LocalCorpus source does not exist: {}", self.src.display());
        }

        self.walk_dir(&self.src, &mut files)?;
        Ok(files)
    }

    /// Recursively walk a directory and collect matching files.
    fn walk_dir(&self, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                self.walk_dir(&path, files)?;
            } else if path.is_file() {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| format!(".{e}"))
                    .unwrap_or_default();

                if self.file_extensions.is_empty() || self.file_extensions.contains(&ext) {
                    files.push(path);
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Corpus for LocalCorpus {
    fn name(&self) -> &str {
        &self.name
    }

    async fn prepare(&self, dest: &Path) -> Result<Vec<PathBuf>> {
        let source_files = self.discover_files()?;
        let mut copied = Vec::new();

        for src_file in source_files {
            // Compute relative path from src root
            let rel_path = src_file
                .strip_prefix(&self.src)
                .map_err(|e| anyhow::anyhow!("Failed to strip prefix: {e}"))?;

            let dest_file = dest.join(rel_path);

            // Create parent directories
            if let Some(parent) = dest_file.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }

            // Copy file
            tokio::fs::copy(&src_file, &dest_file).await?;
            copied.push(rel_path.to_path_buf());
        }

        Ok(copied)
    }
}

/// Git corpus — clones a repository at runtime with optional sparse checkout.
pub struct GitCorpus {
    /// Git URL to clone.
    url: String,

    /// Sparse checkout paths (e.g., ["src/vs/editor"]).
    sparse_paths: Vec<String>,

    /// File extensions to include.
    file_extensions: Vec<String>,

    /// Corpus name.
    name: String,
}

impl GitCorpus {
    /// Create a new GitCorpus.
    pub fn new(
        name: String,
        url: String,
        sparse_paths: Vec<String>,
        file_extensions: Vec<String>,
    ) -> Self {
        Self {
            url,
            sparse_paths,
            file_extensions,
            name,
        }
    }

    /// Clone the repository with --depth 1 and optional sparse checkout.
    async fn clone_repo(&self, dest: &Path) -> Result<()> {
        // Use std::process::Command for git (no git2 dependency)
        let mut cmd = tokio::process::Command::new("git");
        cmd.arg("clone")
            .arg("--depth")
            .arg("1")
            .arg("--filter=blob:none")
            .arg("--sparse");

        if !self.sparse_paths.is_empty() {
            cmd.arg("--sparse");
        }

        cmd.arg(&self.url).arg(dest);

        let status = cmd.status().await?;
        if !status.success() {
            anyhow::bail!("git clone failed for {}", self.url);
        }

        // Set sparse checkout paths if specified
        if !self.sparse_paths.is_empty() {
            let mut sparse_cmd = tokio::process::Command::new("git");
            sparse_cmd
                .current_dir(dest)
                .arg("sparse-checkout")
                .arg("set");

            for path in &self.sparse_paths {
                sparse_cmd.arg(path);
            }

            let status = sparse_cmd.status().await?;
            if !status.success() {
                anyhow::bail!("git sparse-checkout failed for {}", self.url);
            }
        }

        Ok(())
    }

    /// Discover files matching the extension filter after clone.
    fn discover_files(&self, dest: &Path) -> Result<Vec<PathBuf>> {
        let mut files = Vec::new();
        self.walk_dir(dest, &mut files)?;
        Ok(files)
    }

    /// Recursively walk a directory and collect matching files.
    fn walk_dir(&self, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();

            // Skip .git directory
            if path.is_dir() {
                if path.file_name().map(|n| n == ".git").unwrap_or(false) {
                    continue;
                }
                self.walk_dir(&path, files)?;
            } else if path.is_file() {
                let ext = path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| format!(".{e}"))
                    .unwrap_or_default();

                if self.file_extensions.is_empty() || self.file_extensions.contains(&ext) {
                    files.push(path);
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Corpus for GitCorpus {
    fn name(&self) -> &str {
        &self.name
    }

    async fn prepare(&self, dest: &Path) -> Result<Vec<PathBuf>> {
        self.clone_repo(dest).await?;
        let files = self.discover_files(dest)?;

        // Return relative paths
        let relative: Vec<PathBuf> = files
            .into_iter()
            .filter_map(|f| f.strip_prefix(dest).ok().map(|p| p.to_path_buf()))
            .collect();

        Ok(relative)
    }
}

/// Multi-corpus — combines multiple corpora into one (for mini-corpus with multiple repos).
pub struct MultiCorpus {
    name: String,
    corpora: Vec<Box<dyn Corpus>>,
}

impl MultiCorpus {
    /// Create a new MultiCorpus.
    pub fn new(name: String, corpora: Vec<Box<dyn Corpus>>) -> Self {
        Self { name, corpora }
    }
}

#[async_trait]
impl Corpus for MultiCorpus {
    fn name(&self) -> &str {
        &self.name
    }

    async fn prepare(&self, dest: &Path) -> Result<Vec<PathBuf>> {
        let mut all_files = Vec::new();

        for (idx, corpus) in self.corpora.iter().enumerate() {
            // Create a subdirectory for each corpus to avoid conflicts
            let corpus_dest = dest.join(format!("corpus_{}", idx));
            tokio::fs::create_dir_all(&corpus_dest).await?;

            let files = corpus.prepare(&corpus_dest).await?;

            // Convert paths to be relative to the main dest
            for file in files {
                let full_path = corpus_dest.join(&file);
                let rel_to_dest = full_path
                    .strip_prefix(dest)
                    .map_err(|e| anyhow::anyhow!("Failed to strip prefix: {e}"))?
                    .to_path_buf();
                all_files.push(rel_to_dest);
            }
        }

        Ok(all_files)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_local_corpus_prepare() {
        // Create a temporary source directory with test files
        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();

        // Create test files
        tokio::fs::write(src_path.join("a.rs"), "fn main() {}")
            .await
            .unwrap();
        tokio::fs::write(src_path.join("b.ts"), "export const x = 1;")
            .await
            .unwrap();
        tokio::fs::write(src_path.join("c.py"), "def foo(): pass")
            .await
            .unwrap();
        tokio::fs::write(src_path.join("d.txt"), "ignore me")
            .await
            .unwrap();

        let corpus = LocalCorpus::new(
            "test".to_string(),
            src_path.to_path_buf(),
            vec![".rs".to_string(), ".ts".to_string(), ".py".to_string()],
        );

        let dest_dir = TempDir::new().unwrap();
        let files = corpus.prepare(dest_dir.path()).await.unwrap();

        assert_eq!(files.len(), 3, "Should copy 3 files (excluding .txt)");
        assert!(files.iter().any(|f| f.to_str().unwrap().ends_with("a.rs")));
        assert!(files.iter().any(|f| f.to_str().unwrap().ends_with("b.ts")));
        assert!(files.iter().any(|f| f.to_str().unwrap().ends_with("c.py")));

        // Verify files were actually copied
        assert!(dest_dir.path().join("a.rs").exists());
        assert!(dest_dir.path().join("b.ts").exists());
        assert!(dest_dir.path().join("c.py").exists());
        assert!(!dest_dir.path().join("d.txt").exists());
    }

    #[tokio::test]
    async fn test_local_corpus_preserves_structure() {
        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();

        // Create nested structure
        tokio::fs::create_dir_all(src_path.join("src/nested"))
            .await
            .unwrap();
        tokio::fs::write(src_path.join("src/a.rs"), "mod a;")
            .await
            .unwrap();
        tokio::fs::write(src_path.join("src/nested/b.rs"), "mod b;")
            .await
            .unwrap();

        let corpus = LocalCorpus::new(
            "test".to_string(),
            src_path.to_path_buf(),
            vec![".rs".to_string()],
        );

        let dest_dir = TempDir::new().unwrap();
        let files = corpus.prepare(dest_dir.path()).await.unwrap();

        assert_eq!(files.len(), 2);
        assert!(dest_dir.path().join("src/a.rs").exists());
        assert!(dest_dir.path().join("src/nested/b.rs").exists());
    }

    #[tokio::test]
    async fn test_local_corpus_empty_filter_includes_all() {
        let src_dir = TempDir::new().unwrap();
        let src_path = src_dir.path();

        tokio::fs::write(src_path.join("a.rs"), "fn main() {}")
            .await
            .unwrap();
        tokio::fs::write(src_path.join("b.txt"), "text")
            .await
            .unwrap();

        let corpus = LocalCorpus::new("test".to_string(), src_path.to_path_buf(), vec![]);

        let dest_dir = TempDir::new().unwrap();
        let files = corpus.prepare(dest_dir.path()).await.unwrap();

        assert_eq!(files.len(), 2, "Empty filter should include all files");
    }

    #[tokio::test]
    async fn test_local_corpus_nonexistent_source_errors() {
        let corpus = LocalCorpus::new(
            "test".to_string(),
            PathBuf::from("/nonexistent/path"),
            vec![".rs".to_string()],
        );

        let dest_dir = TempDir::new().unwrap();
        let result = corpus.prepare(dest_dir.path()).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("does not exist"));
    }
}
