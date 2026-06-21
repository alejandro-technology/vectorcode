//! Security audit — CLI tests (phase-4.2).
//!
//! Enforces REQ-SEC-04: `vectorcode outline` and `vectorcode index --file`
//! must reject paths that fall outside the project root.
//!
//! **Strict TDD — RED at C1**: the current CLI has no boundary check, so
//! `outline /etc/passwd` succeeds (reading the file). C2 wires
//! `resolve_within_project` into both commands to make these tests green.

use assert_cmd::Command;
use std::fs;

/// Build a `vectorcode` command with `current_dir` set to `dir`.
fn vc() -> Command {
    Command::cargo_bin("vectorcode").unwrap()
}

/// Initialize a minimal vectorcode project in `dir` so CLI commands that
/// require `.vectorcode/` don't bail early.
fn init_minimal_project(dir: &std::path::Path) {
    let vc_dir = dir.join(".vectorcode");
    fs::create_dir_all(&vc_dir).unwrap();
    fs::write(vc_dir.join("config.toml"), "[provider]\nname = \"mock\"\n").unwrap();
    fs::write(vc_dir.join(".gitignore"), "index.db\n").unwrap();

    let db = vectorcode::Database::open_in_memory().unwrap();
    db.init_schema(384).unwrap();

    let db_path = vc_dir.join("index.db");
    let file_db = vectorcode::Database::open(&db_path).unwrap();
    file_db.init_schema(384).unwrap();

    let meta = vectorcode::IndexMeta {
        provider: "mock".to_string(),
        model: "mock-embedder".to_string(),
        dimensions: 384,
        created_at: "2026-06-10T20:00:00Z".to_string(),
        last_sync_at: Some("2026-06-10T20:05:00Z".to_string()),
        files_indexed: 0,
        chunks_stored: 0,
        vectorcode_version: "0.1.0".to_string(),
    };
    vectorcode::store::meta::write_index_meta(file_db.conn(), &meta).unwrap();
}

/// Test 19: `vectorcode outline <absolute-outside-path>` must fail
/// (non-zero exit) and print a boundary message.
#[test]
fn cli_outline_rejects_absolute_path_outside_project() {
    let project = tempfile::tempdir().unwrap();
    init_minimal_project(project.path());

    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("target.rs");
    fs::write(&outside_file, "fn outside() {}").unwrap();

    vc().arg("outline")
        .arg(outside_file.to_str().unwrap())
        .current_dir(project.path())
        .assert()
        .failure();
}

/// Test 19b: `vectorcode outline ../../escape` (relative path that escapes)
/// must fail.
#[test]
fn cli_outline_rejects_relative_path_escape() {
    let project = tempfile::tempdir().unwrap();
    init_minimal_project(project.path());

    vc().arg("outline")
        .arg("../../../etc/passwd")
        .current_dir(project.path())
        .assert()
        .failure();
}

/// Test 20: `vectorcode index --file <outside-path>` must fail.
#[test]
fn cli_index_file_rejects_absolute_path_outside_project() {
    let project = tempfile::tempdir().unwrap();
    init_minimal_project(project.path());

    let outside = tempfile::tempdir().unwrap();
    let outside_file = outside.path().join("target.rs");
    fs::write(&outside_file, "fn outside() {}").unwrap();

    vc().arg("index")
        .arg("--file")
        .arg(outside_file.to_str().unwrap())
        .current_dir(project.path())
        .assert()
        .failure();
}

/// Test 20b: `vectorcode index --file ../../escape` must fail.
#[test]
fn cli_index_file_rejects_relative_path_escape() {
    let project = tempfile::tempdir().unwrap();
    init_minimal_project(project.path());

    vc().arg("index")
        .arg("--file")
        .arg("../../../etc/passwd")
        .current_dir(project.path())
        .assert()
        .failure();
}
