//! Path-safety primitives for boundary enforcement (phase-4.2 security audit).
//!
//! This module is the single home for path-resolution helpers used by MCP
//! handlers, CLI commands, and the indexer. It canonicalizes user-supplied
//! paths and verifies they fall inside an authorized workspace root.
//!
//! ## Module status
//!
//! **STUB SCAFFOLDING (C1)**: Functions in this file return error variants
//! so the red-first test suite can compile and fail. The real implementation
//! arrives in C2. The stubs intentionally use only `Err(...)` returns — no
//! `unwrap`/`expect` in library code (enforced by `security_audit_config`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::error::VectorCodeError;
use crate::mcp::AppInnerState;

/// Resolve a user-supplied path against the set of initialized workspaces.
///
/// Returns the canonical file path and a reference to the owning
/// `AppInnerState`. Iterates workspaces in `BTreeMap` order so the
/// resolution is deterministic when multiple workspaces contain the file.
///
/// **Stub**: always returns `PathOutsideAnyWorkspace`.
pub fn resolve_within_workspace<'a>(
    _raw_path: &str,
    _workspaces: &'a BTreeMap<PathBuf, AppInnerState>,
) -> Result<(PathBuf, &'a AppInnerState), VectorCodeError> {
    Err(VectorCodeError::PathOutsideAnyWorkspace {
        path: String::new(),
    })
}

/// Resolve a user-supplied path against a single project root (CLI variant).
///
/// Used by `outline` and `index --file` which have exactly one project root.
/// Rejects paths that, after canonicalization, fall outside `project_root`.
///
/// **Stub**: always returns `PathOutsideAnyWorkspace`.
pub fn resolve_within_project(
    _path: &str,
    _project_root: &Path,
) -> Result<PathBuf, VectorCodeError> {
    Err(VectorCodeError::PathOutsideAnyWorkspace {
        path: String::new(),
    })
}

/// Canonicalize a path, falling back to the input on error.
///
/// Thin wrapper reserved for the C2 implementation. The current stub just
/// delegates to `std::fs::canonicalize` so tests that exercise only this
/// function can already pass.
pub fn canonicalize_within(base: &Path) -> std::io::Result<PathBuf> {
    std::fs::canonicalize(base)
}

/// Test-only: return `true` when `path` is inside `root` after canonicalization.
///
/// Exposed for the `security_audit_mcp` integration suite and for inline
/// unit tests below. Not gated by `#[cfg(test)]` because integration tests
/// in `tests/` don't have that attribute applied.
///
/// **Stub**: always returns `false`.
pub fn boundary_check(_path: &Path, _root: &Path) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_within_workspace_stub_returns_err() {
        let workspaces: BTreeMap<PathBuf, AppInnerState> = BTreeMap::new();
        let result = resolve_within_workspace("any/path", &workspaces);
        assert!(result.is_err(), "stub should return error");
    }

    #[test]
    fn resolve_within_project_stub_returns_err() {
        let root = Path::new("/tmp");
        let result = resolve_within_project("any/path", root);
        assert!(result.is_err(), "stub should return error");
    }

    #[test]
    fn canonicalize_within_delegates_to_std() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = canonicalize_within(dir.path()).unwrap();
        assert_eq!(canonical, dir.path().canonicalize().unwrap());
    }
}
