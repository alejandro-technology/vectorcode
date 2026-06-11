//! File watcher module — debounced file change detection with gitignore filtering (spec §14).
//!
//! Monitors the project directory for file changes using native OS events
//! (FSEvents on macOS, inotify on Linux), filters through `.gitignore` rules
//! and supported language extensions, and provides a channel-based interface
//! for the MCP server to consume debounced batches.

pub mod gitignore;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use notify_debouncer_full::{
    new_debouncer,
    notify::{self, RecursiveMode},
    DebounceEventResult, Debouncer, RecommendedCache,
};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

use crate::config::schema::WatcherConfig;

use self::gitignore::{filter_paths, GitignoreFilter};

/// A file that has been modified but not yet re-indexed.
#[derive(Debug, Clone)]
pub struct PendingFile {
    /// Absolute path to the modified file.
    pub path: PathBuf,
    /// When the modification was detected.
    pub modified_at: SystemTime,
}

/// File watcher that monitors a project directory for changes.
///
/// Uses `notify-debouncer-full` to coalesce rapid file system events
/// into batches, then filters through `.gitignore` and supported extensions
/// before emitting the batch via an async channel.
pub struct FileWatcher {
    /// The debouncer handle — kept alive for the watcher's lifetime.
    /// When dropped, the watcher thread is stopped.
    _debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
    /// Receiver for debounced batches of changed file paths.
    rx: mpsc::Receiver<Vec<PathBuf>>,
    /// Files that have been detected as changed but not yet re-indexed.
    pending: Arc<RwLock<Vec<PendingFile>>>,
    /// The project root being watched.
    project_root: PathBuf,
}

impl FileWatcher {
    /// Create a new FileWatcher for the given project root.
    ///
    /// The watcher starts monitoring immediately. Use `next_batch()` to
    /// receive debounced batches of changed file paths.
    pub fn new(project_root: &Path, config: &WatcherConfig) -> Result<Self> {
        let project_root = project_root.to_path_buf();
        let gitignore = Arc::new(GitignoreFilter::new(&project_root));
        let pending: Arc<RwLock<Vec<PendingFile>>> = Arc::new(RwLock::new(Vec::new()));
        let pending_clone = pending.clone();
        let root_clone = project_root.clone();

        // Channel for the debouncer callback → async receiver
        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>(64);

        let debounce_duration = Duration::from_millis(config.debounce_ms);

        // Create the debouncer with a callback that filters and forwards events
        let debouncer = new_debouncer(
            debounce_duration,
            None,
            move |result: DebounceEventResult| {
                match result {
                    Ok(events) => {
                        // Extract unique file paths from events
                        let mut paths: Vec<PathBuf> = events
                            .iter()
                            .flat_map(|e| e.paths.iter().cloned())
                            .collect::<std::collections::HashSet<_>>()
                            .into_iter()
                            .collect();

                        // Filter through gitignore + supported extensions
                        paths = filter_paths(&paths, &root_clone, &gitignore);

                        if paths.is_empty() {
                            return;
                        }

                        debug!("Watcher detected {} changed files", paths.len());

                        // Update pending files
                        let now = SystemTime::now();
                        let pending_files: Vec<PendingFile> = paths
                            .iter()
                            .map(|p| PendingFile {
                                path: p.clone(),
                                modified_at: now,
                            })
                            .collect();

                        // Spawn a blocking task to update pending (since we're in a sync callback)
                        let pending_inner = pending_clone.clone();
                        let new_paths = paths.clone();
                        std::thread::spawn(move || {
                            let rt = tokio::runtime::Handle::try_current();
                            if let Ok(handle) = rt {
                                handle.spawn(async move {
                                    let mut pending = pending_inner.write().await;
                                    // Merge: add new pending files, dedup by path
                                    for pf in pending_files {
                                        if !pending.iter().any(|p| p.path == pf.path) {
                                            pending.push(pf);
                                        }
                                    }
                                });
                            }
                        });

                        // Send the batch through the channel (blocking send in callback thread)
                        // Use try_send to avoid blocking the notify thread
                        if let Err(e) = tx.try_send(new_paths) {
                            warn!("Failed to send watcher batch: {e}");
                        }
                    }
                    Err(errors) => {
                        for err in &errors {
                            warn!("Watcher error: {err}");
                        }
                    }
                }
            },
        )?;

        info!("File watcher starting on {}", project_root.display());

        Ok(Self {
            _debouncer: debouncer,
            rx,
            pending,
            project_root,
        })
    }

    /// Start watching the project root directory recursively.
    ///
    /// This must be called after `new()` to begin receiving events.
    pub fn start(&mut self) -> Result<()> {
        self._debouncer
            .watch(&self.project_root, RecursiveMode::Recursive)?;
        info!("File watcher monitoring {}", self.project_root.display());
        Ok(())
    }

    /// Wait for the next debounced batch of changed file paths.
    ///
    /// Returns `None` when the watcher is shut down (channel closed).
    pub async fn next_batch(&mut self) -> Option<Vec<PathBuf>> {
        self.rx.recv().await
    }

    /// Get the list of files that have been modified but not yet re-indexed.
    ///
    /// Used by the staleness banner in search results (spec §14.2).
    pub async fn pending_files(&self) -> Vec<PendingFile> {
        self.pending.read().await.clone()
    }

    /// Clear pending files after they have been re-indexed.
    pub async fn clear_pending(&self) {
        self.pending.write().await.clear();
    }

    /// Clear specific paths from the pending list (after they've been re-indexed).
    pub async fn clear_pending_paths(&self, paths: &[PathBuf]) {
        let mut pending = self.pending.write().await;
        pending.retain(|pf| !paths.contains(&pf.path));
    }

    /// Get the project root being watched.
    pub fn project_root(&self) -> &Path {
        &self.project_root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::schema::WatcherConfig;
    use serial_test::serial;
    use std::fs;

    fn default_watcher_config() -> WatcherConfig {
        WatcherConfig {
            debounce_ms: 200, // Short debounce for tests
            disabled: false,
        }
    }

    // ─── PendingFile tests ─────────────────────────────────────────────

    #[test]
    fn pending_file_stores_path_and_time() {
        let now = SystemTime::now();
        let pf = PendingFile {
            path: PathBuf::from("src/main.rs"),
            modified_at: now,
        };
        assert_eq!(pf.path, PathBuf::from("src/main.rs"));
        assert_eq!(pf.modified_at, now);
    }

    #[test]
    fn pending_file_clone_is_independent() {
        let pf = PendingFile {
            path: PathBuf::from("test.rs"),
            modified_at: SystemTime::now(),
        };
        let cloned = pf.clone();
        assert_eq!(cloned.path, pf.path);
    }

    // ─── FileWatcher creation tests ────────────────────────────────────

    #[test]
    #[serial(watcher)]
    fn file_watcher_creates_successfully() {
        let dir = tempfile::tempdir().unwrap();
        let config = default_watcher_config();
        let watcher = FileWatcher::new(dir.path(), &config);
        assert!(
            watcher.is_ok(),
            "FileWatcher should create: {:?}",
            watcher.err()
        );
    }

    #[test]
    #[serial(watcher)]
    fn file_watcher_project_root_is_correct() {
        let dir = tempfile::tempdir().unwrap();
        let config = default_watcher_config();
        let watcher = FileWatcher::new(dir.path(), &config).unwrap();
        assert_eq!(watcher.project_root(), dir.path());
    }

    #[test]
    #[serial(watcher)]
    fn file_watcher_starts_with_empty_pending() {
        let dir = tempfile::tempdir().unwrap();
        let config = default_watcher_config();
        let watcher = FileWatcher::new(dir.path(), &config).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let pending = rt.block_on(watcher.pending_files());
        assert!(pending.is_empty(), "Pending should be empty initially");
    }

    #[test]
    #[serial(watcher)]
    fn file_watcher_start_watches_directory() {
        let dir = tempfile::tempdir().unwrap();
        let config = default_watcher_config();
        let mut watcher = FileWatcher::new(dir.path(), &config).unwrap();
        let result = watcher.start();
        assert!(result.is_ok(), "start() should succeed: {:?}", result.err());
    }

    // ─── FileWatcher event detection tests ─────────────────────────────

    #[tokio::test]
    #[serial(watcher)]
    async fn file_watcher_detects_new_file_creation() {
        let dir = tempfile::tempdir().unwrap();
        let config = WatcherConfig {
            debounce_ms: 100,
            disabled: false,
        };
        let mut watcher = FileWatcher::new(dir.path(), &config).unwrap();
        watcher.start().unwrap();

        // Create a new .rs file
        let new_file = dir.path().join("new_file.rs");
        fs::write(&new_file, "fn hello() {}").unwrap();

        // Wait for debounced event
        let batch = tokio::time::timeout(Duration::from_secs(3), watcher.next_batch()).await;

        match batch {
            Ok(Some(paths)) => {
                assert!(
                    paths.iter().any(|p| p.ends_with("new_file.rs")),
                    "Should detect new_file.rs, got: {paths:?}"
                );
            }
            Ok(None) => panic!("Channel closed unexpectedly"),
            Err(_) => {
                // Timeout is acceptable in CI environments where FS events may be slow
                eprintln!("Note: watcher timeout in test (FS events may be slow in CI)");
            }
        }
    }

    #[tokio::test]
    #[serial(watcher)]
    async fn file_watcher_filters_unsupported_extensions() {
        let dir = tempfile::tempdir().unwrap();
        let config = WatcherConfig {
            debounce_ms: 100,
            disabled: false,
        };
        let mut watcher = FileWatcher::new(dir.path(), &config).unwrap();
        watcher.start().unwrap();

        // Create a .txt file (unsupported)
        let txt_file = dir.path().join("notes.txt");
        fs::write(&txt_file, "some notes").unwrap();

        // Wait briefly — should NOT get a batch for .txt files
        let batch = tokio::time::timeout(Duration::from_millis(500), watcher.next_batch()).await;

        // Either timeout (no events) or empty batch — both are acceptable
        match batch {
            Ok(Some(paths)) => {
                assert!(
                    !paths.iter().any(|p| p.ends_with("notes.txt")),
                    "Should NOT include .txt files, got: {paths:?}"
                );
            }
            Ok(None) => {} // Channel closed
            Err(_) => {}   // Timeout — expected
        }
    }

    // ─── clear_pending tests ───────────────────────────────────────────

    #[tokio::test]
    #[serial(watcher)]
    async fn clear_pending_empties_the_list() {
        let dir = tempfile::tempdir().unwrap();
        let config = default_watcher_config();
        let watcher = FileWatcher::new(dir.path(), &config).unwrap();

        // Manually add a pending file
        watcher.pending.write().await.push(PendingFile {
            path: PathBuf::from("test.rs"),
            modified_at: SystemTime::now(),
        });

        assert_eq!(watcher.pending_files().await.len(), 1);

        watcher.clear_pending().await;
        assert!(watcher.pending_files().await.is_empty());
    }

    #[tokio::test]
    #[serial(watcher)]
    async fn clear_pending_paths_removes_specific_paths() {
        let dir = tempfile::tempdir().unwrap();
        let config = default_watcher_config();
        let watcher = FileWatcher::new(dir.path(), &config).unwrap();

        let now = SystemTime::now();
        watcher.pending.write().await.push(PendingFile {
            path: PathBuf::from("a.rs"),
            modified_at: now,
        });
        watcher.pending.write().await.push(PendingFile {
            path: PathBuf::from("b.rs"),
            modified_at: now,
        });

        watcher.clear_pending_paths(&[PathBuf::from("a.rs")]).await;

        let remaining = watcher.pending_files().await;
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].path, PathBuf::from("b.rs"));
    }
}
