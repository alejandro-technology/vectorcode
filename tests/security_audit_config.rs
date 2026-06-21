//! Security audit — config / static-analysis tests (phase-4.2).
//!
//! Enforces REQ-SEC-06: no `.unwrap()` or `.expect(` calls in library code
//! (i.e. under `src/`, outside of `#[cfg(test)]` modules).
//!
//! **Strict TDD — RED at C1**: current library code contains unwraps, so the
//! scan finds violations and the assertion fails. C2 removes the unwraps to
//! turn the test green.

use std::fs;
use std::path::Path;

/// Walk `src/**/*.rs`, check that non-test regions contain no
/// `.unwrap()` or `.expect(` calls.
#[test]
fn no_unwrap_or_expect_in_library_code() {
    let src_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations: Vec<String> = Vec::new();
    scan_dir(&src_dir, &mut violations);

    assert!(
        violations.is_empty(),
        "Found {} unwrap/expect call(s) in library code:\n  {}",
        violations.len(),
        violations.join("\n  ")
    );
}

fn scan_dir(dir: &Path, violations: &mut Vec<String>) {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_dir(&path, violations);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            scan_file(&path, violations);
        }
    }
}

/// Scan a single `.rs` file. Tracks `#[cfg(test)]` module boundaries so
/// `unwrap` inside test modules is permitted (and expected).
fn scan_file(path: &Path, violations: &mut Vec<String>) {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };

    let mut in_test_module = false;
    let mut brace_depth: i32 = 0;
    let mut in_cfg_test_attr = false;

    for (idx, line) in content.lines().enumerate() {
        let trimmed = line.trim();

        // Detect `#[cfg(test)]` attribute line.
        if trimmed.starts_with("#[cfg(test)]") || trimmed.contains("#[cfg(test)]") {
            in_cfg_test_attr = true;
        }

        // Track entering a test module: the brace opening on the next line.
        if in_cfg_test_attr && trimmed.contains('{') {
            in_test_module = true;
            in_cfg_test_attr = false;
        }

        if !in_test_module && (trimmed.contains(".unwrap()") || trimmed.contains(".expect(")) {
            violations.push(format!("{}:{}: {}", path.display(), idx + 1, trimmed));
        }

        // Track brace depth to leave the test module.
        brace_depth += trimmed.matches('{').count() as i32;
        brace_depth -= trimmed.matches('}').count() as i32;
        if in_test_module && brace_depth <= 0 {
            in_test_module = false;
            brace_depth = 0;
        }
    }
}
