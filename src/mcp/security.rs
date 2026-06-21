//! Path-safety primitives for boundary enforcement (phase-4.2 security audit).
//!
//! This module is the single home for path-resolution helpers used by MCP
//! handlers, CLI commands, and the indexer. It canonicalizes user-supplied
//! paths and verifies they fall inside an authorized workspace root.
//!
//! ## Guarantees
//!
//! - `resolve_within_workspace` iterates workspaces in `BTreeMap` order
//!   (lexicographic) so resolution is deterministic when multiple
//!   workspaces contain the same file (R7).
//! - All public functions canonicalize both the input path and the
//!   candidate root before the `starts_with` check, so alias forms like
//!   `../repo` or symlinks pointing inside the root resolve correctly.
//! - Failure modes map to a single error variant, `PathOutsideAnyWorkspace`,
//!   so callers can distinguish "denied" from generic I/O failures.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::VectorCodeError;
use crate::mcp::AppInnerState;

/// Resolve a user-supplied path against the set of initialized workspaces.
///
/// Returns the canonical file path and a reference to the owning
/// `AppInnerState`. The first workspace (in `BTreeMap` order) whose
/// canonical root is an ancestor of the canonical file path wins.
///
/// Errors:
/// - `PathOutsideAnyWorkspace` when the canonicalized file does not fall
///   under any registered workspace root.
pub fn resolve_within_workspace<'a>(
    raw_path: &str,
    workspaces: &'a BTreeMap<PathBuf, AppInnerState>,
) -> Result<(PathBuf, &'a AppInnerState), VectorCodeError> {
    // Canonicalize each workspace root once.
    let mut canonical_roots: Vec<(PathBuf, &'a AppInnerState)> =
        Vec::with_capacity(workspaces.len());
    for (root, state) in workspaces {
        let canonical_root = canonicalize_within(root).unwrap_or_else(|_| root.clone());
        canonical_roots.push((canonical_root, state));
    }
    // BTreeMap iteration is already lexicographic, so this sort is a no-op
    // on the ordering but keeps the contract explicit.
    canonical_roots.sort_by(|a, b| a.0.cmp(&b.0));

    for (canonical_root, state) in &canonical_roots {
        let candidate = canonical_root.join(raw_path);
        // Try to canonicalize the candidate. If the file does not exist
        // we skip this root (the file might exist under a different root).
        let canonical_candidate = match canonicalize_within(&candidate) {
            Ok(p) => p,
            Err(_) => continue,
        };
        if boundary_check(&canonical_candidate, canonical_root) {
            return Ok((canonical_candidate, state));
        }
    }

    Err(VectorCodeError::PathOutsideAnyWorkspace {
        path: raw_path.to_string(),
    })
}

/// Resolve a user-supplied path against a single project root (CLI variant).
///
/// Used by `outline` and `index --file` which have exactly one project root.
/// Rejects paths that, after canonicalization, fall outside `project_root`.
pub fn resolve_within_project(path: &str, project_root: &Path) -> Result<PathBuf, VectorCodeError> {
    let canonical_root =
        canonicalize_within(project_root).unwrap_or_else(|_| project_root.to_path_buf());
    let candidate = if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else {
        canonical_root.join(path)
    };
    let canonical =
        canonicalize_within(&candidate).map_err(|_| VectorCodeError::PathOutsideAnyWorkspace {
            path: path.to_string(),
        })?;
    if boundary_check(&canonical, &canonical_root) {
        Ok(canonical)
    } else {
        Err(VectorCodeError::PathOutsideAnyWorkspace {
            path: path.to_string(),
        })
    }
}

/// Canonicalize a path, falling back to the input on error.
pub fn canonicalize_within(base: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(base)
}

/// Return `true` when `path` is inside `root` after canonicalization.
///
/// Pure helper exposed for testing and for callers that already have
/// canonicalized paths on hand.
pub fn boundary_check(path: &Path, root: &Path) -> bool {
    let canonical_path = match canonicalize_within(path) {
        Ok(p) => p,
        Err(_) => return false,
    };
    let canonical_root = match canonicalize_within(root) {
        Ok(p) => p,
        Err(_) => return false,
    };
    canonical_path.starts_with(&canonical_root)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boundary_check_inside_returns_true() {
        let dir = tempfile::tempdir().unwrap();
        let inside = dir.path().join("inside.rs");
        std::fs::write(&inside, "fn inside() {}").unwrap();
        assert!(boundary_check(&inside, dir.path()));
    }

    #[test]
    fn boundary_check_outside_returns_false() {
        let dir = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("outside.rs");
        std::fs::write(&outside, "fn outside() {}").unwrap();
        assert!(!boundary_check(&outside, dir.path()));
    }

    #[test]
    fn canonicalize_within_resolves_canonical_path() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = canonicalize_within(dir.path()).unwrap();
        assert_eq!(canonical, dir.path().canonicalize().unwrap());
    }
}
