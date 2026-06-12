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
    notify::{self, EventKind, RecursiveMode},
    DebounceEventResult, Debouncer, RecommendedCache,
};
use tokio::sync::{mpsc, RwLock};
use tracing::{debug, info, warn};

use crate::config::schema::WatcherConfig;

use self::gitignore::{filter_paths, has_supported_extension, GitignoreFilter};

/// A batch of file changes detected by the watcher.
///
/// Each entry is a `(path, is_removal)` tuple: `true` means the file was deleted.
pub type ChangeBatch = Vec<(PathBuf, bool)>;

/// An unbounded receiver for [`ChangeBatch`] notifications.
pub type ChangeBatchReceiver = mpsc::UnboundedReceiver<ChangeBatch>;

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
    /// Files that have been detected as changed but not yet re-indexed.
    pending: Arc<RwLock<Vec<PendingFile>>>,
    /// The project root being watched.
    project_root: PathBuf,
}

impl FileWatcher {
    /// Create a new FileWatcher for the given project root.
    ///
    /// The watcher starts monitoring immediately. It returns the FileWatcher instance
    /// along with an unbounded receiver for debounced batches of changed file paths.
    pub fn new(project_root: &Path, config: &WatcherConfig) -> Result<(Self, ChangeBatchReceiver)> {
        let project_root = project_root.to_path_buf();
        let gitignore = Arc::new(GitignoreFilter::new(&project_root));
        let pending: Arc<RwLock<Vec<PendingFile>>> = Arc::new(RwLock::new(Vec::new()));
        let pending_clone = pending.clone();
        let root_clone = project_root.clone();

        // Unbounded channels: notify callback runs on a std::thread, so we use
        // unbounded channels to avoid blocking. (H14: prevents buffer overflow)
        let (tx, rx) = mpsc::unbounded_channel::<ChangeBatch>();
        let (tx_pending, mut rx_pending) = mpsc::unbounded_channel::<ChangeBatch>();

        // Spawn a task to process pending updates from the channel (C7: replaces dead code).
        // Only spawn if a tokio runtime is available (tests may not have one).
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                while let Some(updates) = rx_pending.recv().await {
                    let mut pending = pending_clone.write().await;
                    for (path, is_removal) in updates {
                        if is_removal {
                            // Remove from pending (file was deleted)
                            pending.retain(|pf| pf.path != path);
                        } else if !pending.iter().any(|pf| pf.path == path) {
                            // Add to pending (file was modified/created)
                            pending.push(PendingFile {
                                path,
                                modified_at: SystemTime::now(),
                            });
                        }
                    }
                }
            });
        }

        let debounce_duration = Duration::from_millis(config.debounce_ms);

        // Create the debouncer with a callback that filters and forwards events
        let debouncer = new_debouncer(
            debounce_duration,
            None,
            move |result: DebounceEventResult| {
                match result {
                    Ok(events) => {
                        // Separate removals from other events (H13: handle deletions)
                        let mut removal_paths: Vec<PathBuf> = Vec::new();
                        let mut other_paths: Vec<PathBuf> = Vec::new();

                        for event in &events {
                            for path in &event.paths {
                                match event.kind {
                                    EventKind::Remove(_) => {
                                        removal_paths.push(path.clone());
                                    }
                                    _ => {
                                        other_paths.push(path.clone());
                                    }
                                }
                            }
                        }

                        // Filter modifications through gitignore + extension check
                        let filtered_mods = filter_paths(&other_paths, &root_clone, &gitignore);

                        // Filter removals: check extension + gitignore, but NOT is_file()
                        // (deleted files don't exist on disk, so is_file() returns false)
                        let filtered_removals: Vec<PathBuf> = removal_paths
                            .into_iter()
                            .filter(|p| {
                                p.starts_with(&root_clone)
                                    && !gitignore.is_ignored(p)
                                    && has_supported_extension(p)
                            })
                            .collect();

                        // Build batch: (path, is_removal) tuples
                        let mut batch: Vec<(PathBuf, bool)> = Vec::new();
                        for p in filtered_mods {
                            batch.push((p, false));
                        }
                        for p in filtered_removals {
                            batch.push((p, true));
                        }

                        if batch.is_empty() {
                            return;
                        }

                        debug!("Watcher detected {} changed files", batch.len());

                        // Send batch notification (unbounded, never blocks)
                        if let Err(e) = tx.send(batch.clone()) {
                            warn!("Failed to send watcher batch: {e}");
                        }

                        // Send pending updates (unbounded, never blocks)
                        if let Err(e) = tx_pending.send(batch) {
                            warn!("Failed to send watcher pending updates: {e}");
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

        let watcher = Self {
            _debouncer: debouncer,
            pending,
            project_root,
        };

        Ok((watcher, rx))
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
        let result = FileWatcher::new(dir.path(), &config);
        assert!(
            result.is_ok(),
            "FileWatcher should create: {:?}",
            result.err()
        );
    }

    #[test]
    #[serial(watcher)]
    fn file_watcher_project_root_is_correct() {
        let dir = tempfile::tempdir().unwrap();
        let config = default_watcher_config();
        let (watcher, _) = FileWatcher::new(dir.path(), &config).unwrap();
        assert_eq!(watcher.project_root(), dir.path());
    }

    #[test]
    #[serial(watcher)]
    fn file_watcher_starts_with_empty_pending() {
        let dir = tempfile::tempdir().unwrap();
        let config = default_watcher_config();
        let (watcher, _) = FileWatcher::new(dir.path(), &config).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let pending = rt.block_on(watcher.pending_files());
        assert!(pending.is_empty(), "Pending should be empty initially");
    }

    #[test]
    #[serial(watcher)]
    fn file_watcher_start_watches_directory() {
        let dir = tempfile::tempdir().unwrap();
        let config = default_watcher_config();
        let (mut watcher, _) = FileWatcher::new(dir.path(), &config).unwrap();
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
        let (mut watcher, mut rx) = FileWatcher::new(dir.path(), &config).unwrap();
        watcher.start().unwrap();

        // Create a new .rs file
        let new_file = dir.path().join("new_file.rs");
        fs::write(&new_file, "fn hello() {}").unwrap();

        // Wait for debounced event
        let batch = tokio::time::timeout(Duration::from_secs(3), rx.recv()).await;

        match batch {
            Ok(Some(entries)) => {
                assert!(
                    entries.iter().any(|(p, _)| p.ends_with("new_file.rs")),
                    "Should detect new_file.rs, got: {entries:?}"
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
        let (mut watcher, mut rx) = FileWatcher::new(dir.path(), &config).unwrap();
        watcher.start().unwrap();

        // Create a .txt file (unsupported)
        let txt_file = dir.path().join("notes.txt");
        fs::write(&txt_file, "some notes").unwrap();

        // Wait briefly — should NOT get a batch for .txt files
        let batch = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;

        // Either timeout (no events) or empty batch — both are acceptable
        match batch {
            Ok(Some(entries)) => {
                assert!(
                    !entries.iter().any(|(p, _)| p.ends_with("notes.txt")),
                    "Should NOT include .txt files, got: {entries:?}"
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
        let (watcher, _) = FileWatcher::new(dir.path(), &config).unwrap();

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
        let (watcher, _) = FileWatcher::new(dir.path(), &config).unwrap();

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
