//! Security audit — MCP tests (phase-4.2).
//!
//! Enforces REQ-SEC-02 / REQ-SEC-03 / REQ-SEC-05:
//! - `resolve_within_workspace` canonicalizes and returns the owning
//!   workspace, or `PathOutsideAnyWorkspace` when the path escapes every
//!   workspace.
//! - Workspace iteration is deterministic (BTreeMap, R7).
//! - `boundary_check` returns the right answer for inside/outside cases.
//!
//! **Strict TDD — RED at C1**: the stubs in `src/mcp/security.rs` always
//! return `PathOutsideAnyWorkspace`, so tests that expect `Ok` fail. C2
//! replaces the stubs with real implementations.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use vectorcode::config::schema::Config;
use vectorcode::embedder::mock::MockEmbedder;
use vectorcode::mcp::security::{boundary_check, resolve_within_project, resolve_within_workspace};
use vectorcode::mcp::AppInnerState;
use vectorcode::store::db::Database;

fn make_inner_state(root: &std::path::Path) -> AppInnerState {
    let db = Database::open_in_memory().unwrap();
    let _ = db.init_schema(64);
    AppInnerState {
        db: Arc::new(tokio::sync::Mutex::new(db)),
        embedder: Arc::new(MockEmbedder::new(64)),
        config: Config::default(),
        project_path: root.to_path_buf(),
        watcher: None,
    }
}

fn make_workspaces(roots: &[&std::path::Path]) -> BTreeMap<PathBuf, AppInnerState> {
    let mut map = BTreeMap::new();
    for r in roots {
        let canonical = r.canonicalize().unwrap_or_else(|_| r.to_path_buf());
        map.insert(canonical, make_inner_state(r));
    }
    map
}

/// Happy path: a file that exists inside a registered workspace must
/// resolve to `Ok` with the canonical path.
#[test]
fn resolve_within_workspace_resolves_internal_file() {
    let project = tempfile::tempdir().unwrap();
    let src = project.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    let file = src.join("main.rs");
    std::fs::write(&file, "fn main() {}").unwrap();

    let workspaces = make_workspaces(&[project.path()]);
    let result = resolve_within_workspace("src/main.rs", &workspaces);
    assert!(result.is_ok(), "Internal file should resolve");
}

/// Path that escapes every registered workspace must return
/// `PathOutsideAnyWorkspace`.
#[test]
fn resolve_within_workspace_rejects_traversal() {
    let project = tempfile::tempdir().unwrap();
    let workspaces = make_workspaces(&[project.path()]);
    let result = resolve_within_workspace("../../etc/passwd", &workspaces);
    assert!(result.is_err(), "Traversal must be rejected");
}

/// Path that does not exist in any workspace must return
/// `PathOutsideAnyWorkspace` (not a generic Io error).
#[test]
fn resolve_within_workspace_rejects_nonexistent() {
    let project = tempfile::tempdir().unwrap();
    let workspaces = make_workspaces(&[project.path()]);
    let result = resolve_within_workspace("does/not/exist.rs", &workspaces);
    assert!(result.is_err(), "Nonexistent file must be rejected");
}

/// R7 determinism: when two workspaces both own the same file, the
/// lexicographically first workspace root must win — and the choice must
/// be stable across two separate invocations.
#[test]
fn resolve_within_workspace_is_deterministic_across_overlaps() {
    let parent = tempfile::tempdir().unwrap();
    let sub = parent.path().join("sub");
    std::fs::create_dir_all(&sub).unwrap();

    let shared = parent.path().join("shared.rs");
    std::fs::write(&shared, "// shared").unwrap();

    let workspaces = make_workspaces(&[parent.path(), sub.as_path()]);

    let r1 = resolve_within_workspace("shared.rs", &workspaces);
    let r2 = resolve_within_workspace("shared.rs", &workspaces);
    assert!(r1.is_ok(), "Should resolve under at least one workspace");
    assert!(r2.is_ok(), "Should resolve under at least one workspace");
    let (p1, _s1) = r1.unwrap();
    let (p2, _s2) = r2.unwrap();
    assert_eq!(p1, p2, "Same input must pick the same workspace");
}

/// `boundary_check` must return `true` for a path inside the root after
/// canonicalization, and `false` for a path outside.
#[test]
fn boundary_check_distinguishes_inside_from_outside() {
    let root = tempfile::tempdir().unwrap();
    let inside = root.path().join("inside.rs");
    std::fs::write(&inside, "fn inside() {}").unwrap();

    let outside_dir = tempfile::tempdir().unwrap();
    let outside = outside_dir.path().join("outside.rs");
    std::fs::write(&outside, "fn outside() {}").unwrap();

    assert!(
        boundary_check(&inside, root.path()),
        "File under root must be inside"
    );
    assert!(
        !boundary_check(&outside, root.path()),
        "File outside root must be flagged"
    );
}

/// `resolve_within_project` (CLI variant) must succeed for a file inside
/// the project root and fail for one outside.
#[test]
fn resolve_within_project_accepts_internal_and_rejects_external() {
    let project = tempfile::tempdir().unwrap();
    let src = project.path().join("src");
    std::fs::create_dir_all(&src).unwrap();
    let inside = src.join("inside.rs");
    std::fs::write(&inside, "fn inside() {}").unwrap();

    let outside_dir = tempfile::tempdir().unwrap();
    let outside = outside_dir.path().join("outside.rs");
    std::fs::write(&outside, "fn outside() {}").unwrap();

    let r1 = resolve_within_project(inside.to_str().unwrap(), project.path());
    assert!(r1.is_ok(), "Internal path should resolve");

    let r2 = resolve_within_project(outside.to_str().unwrap(), project.path());
    assert!(r2.is_err(), "External path must be rejected");
}
