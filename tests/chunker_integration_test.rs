/// Integration tests for AST-aware chunking with real fixture files.
use std::fs;
use vectorcode::engine::chunker::chunk_file;
use vectorcode::engine::languages::SupportedLanguage;

#[test]
fn chunk_typescript_fixture() {
    let source = fs::read_to_string("tests/fixtures/sample_ts/calculator.ts")
        .expect("Failed to read TypeScript fixture");
    let chunks = chunk_file(&source, "calculator.ts", SupportedLanguage::TypeScript);

    assert!(
        !chunks.is_empty(),
        "Should produce chunks from TypeScript fixture"
    );
    assert!(
        chunks.len() >= 2,
        "Should produce at least 2 chunks (class + function), got {}",
        chunks.len()
    );

    // Verify chunk metadata
    for chunk in &chunks {
        assert_eq!(chunk.language, "typescript");
        assert_eq!(chunk.file_path, "calculator.ts");
        assert!(chunk.start_line > 0);
        assert!(chunk.end_line >= chunk.start_line);
        assert!(!chunk.content.is_empty());
        assert!(!chunk.id.is_empty());
        assert!(!chunk.content_hash.is_empty());
    }

    // Verify we extracted a class
    let has_class = chunks
        .iter()
        .any(|c| c.kind == "export_statement" && c.content.contains("Calculator"));
    assert!(has_class, "Should have extracted the Calculator class");
}

#[test]
fn chunk_typescript_task_fixture() {
    let source = fs::read_to_string("tests/fixtures/sample_ts/task.ts")
        .expect("Failed to read TypeScript task fixture");
    let chunks = chunk_file(&source, "task.ts", SupportedLanguage::TypeScript);

    assert!(
        !chunks.is_empty(),
        "Should produce chunks from TypeScript task fixture"
    );

    // Verify we have multiple chunks (interface, enum, functions)
    assert!(
        chunks.len() >= 3,
        "Should produce at least 3 chunks, got {}",
        chunks.len()
    );
}

#[test]
fn chunk_python_fixture() {
    let source = fs::read_to_string("tests/fixtures/sample_py/auth.py")
        .expect("Failed to read Python fixture");
    let chunks = chunk_file(&source, "auth.py", SupportedLanguage::Python);

    assert!(
        !chunks.is_empty(),
        "Should produce chunks from Python fixture"
    );
    assert!(
        chunks.len() >= 2,
        "Should produce at least 2 chunks (User class + AuthService class), got {}",
        chunks.len()
    );

    // Verify chunk metadata
    for chunk in &chunks {
        assert_eq!(chunk.language, "python");
        assert_eq!(chunk.file_path, "auth.py");
        assert!(chunk.start_line > 0);
        assert!(chunk.end_line >= chunk.start_line);
        assert!(!chunk.content.is_empty());
    }

    // Verify we extracted classes
    let has_user_class = chunks.iter().any(|c| c.content.contains("class User"));
    let has_auth_service = chunks
        .iter()
        .any(|c| c.content.contains("class AuthService"));
    assert!(
        has_user_class || has_auth_service,
        "Should have extracted at least one class"
    );
}

#[test]
fn chunk_python_pipeline_fixture() {
    let source = fs::read_to_string("tests/fixtures/sample_py/pipeline.py")
        .expect("Failed to read Python pipeline fixture");
    let chunks = chunk_file(&source, "pipeline.py", SupportedLanguage::Python);

    assert!(
        !chunks.is_empty(),
        "Should produce chunks from Python pipeline fixture"
    );

    // Verify we have multiple chunks (functions + class)
    assert!(
        chunks.len() >= 3,
        "Should produce at least 3 chunks, got {}",
        chunks.len()
    );
}

#[test]
fn chunk_rust_fixture() {
    let source = fs::read_to_string("tests/fixtures/sample_rs/store.rs")
        .expect("Failed to read Rust fixture");
    let chunks = chunk_file(&source, "store.rs", SupportedLanguage::Rust);

    assert!(
        !chunks.is_empty(),
        "Should produce chunks from Rust fixture"
    );
    assert!(
        !chunks.is_empty(),
        "Should produce at least 1 chunk (impl), got {}",
        chunks.len()
    );

    // Verify chunk metadata
    for chunk in &chunks {
        assert_eq!(chunk.language, "rust");
        assert_eq!(chunk.file_path, "store.rs");
        assert!(chunk.start_line > 0);
        assert!(chunk.end_line >= chunk.start_line);
        assert!(!chunk.content.is_empty());
    }

    // Verify we extracted impl (struct may be too small and skipped)
    let has_impl = chunks.iter().any(|c| c.kind == "impl_item");
    assert!(has_impl, "Should have extracted impl_item");
}

#[test]
fn chunk_rust_http_fixture() {
    let source = fs::read_to_string("tests/fixtures/sample_rs/http.rs")
        .expect("Failed to read Rust HTTP fixture");
    let chunks = chunk_file(&source, "http.rs", SupportedLanguage::Rust);

    assert!(
        !chunks.is_empty(),
        "Should produce chunks from Rust HTTP fixture"
    );

    // Verify we have multiple chunks (enums, structs, traits, impls)
    assert!(
        chunks.len() >= 5,
        "Should produce at least 5 chunks, got {}",
        chunks.len()
    );
}

#[test]
fn chunk_idempotency() {
    let source = fs::read_to_string("tests/fixtures/sample_ts/calculator.ts")
        .expect("Failed to read TypeScript fixture");

    let chunks1 = chunk_file(&source, "calculator.ts", SupportedLanguage::TypeScript);
    let chunks2 = chunk_file(&source, "calculator.ts", SupportedLanguage::TypeScript);

    assert_eq!(
        chunks1.len(),
        chunks2.len(),
        "Chunking should be deterministic"
    );

    for (c1, c2) in chunks1.iter().zip(chunks2.iter()) {
        assert_eq!(c1.id, c2.id, "Chunk IDs should be identical across runs");
        assert_eq!(
            c1.content_hash, c2.content_hash,
            "Content hashes should be identical"
        );
        assert_eq!(c1.content, c2.content, "Content should be identical");
    }
}

#[test]
fn chunk_different_files_produce_different_ids() {
    let source = fs::read_to_string("tests/fixtures/sample_ts/calculator.ts")
        .expect("Failed to read TypeScript fixture");

    let chunks1 = chunk_file(&source, "file1.ts", SupportedLanguage::TypeScript);
    let chunks2 = chunk_file(&source, "file2.ts", SupportedLanguage::TypeScript);

    assert_eq!(chunks1.len(), chunks2.len());

    // Same content but different file paths should produce different IDs
    for (c1, c2) in chunks1.iter().zip(chunks2.iter()) {
        assert_ne!(
            c1.id, c2.id,
            "Different file paths should produce different chunk IDs"
        );
    }
}

// --- Phase 8: Multi-language integration tests ---

#[test]
fn chunk_csharp_fixture() {
    let source = fs::read_to_string("tests/fixtures/sample_cs/calculator.cs")
        .expect("Failed to read C# fixture");
    let chunks = chunk_file(&source, "calculator.cs", SupportedLanguage::CSharp);

    assert!(!chunks.is_empty(), "Should produce chunks from C# fixture");

    for chunk in &chunks {
        assert_eq!(chunk.language, "csharp");
        assert_eq!(chunk.file_path, "calculator.cs");
        assert!(chunk.start_line > 0);
        assert!(!chunk.content.is_empty());
    }

    let has_class = chunks.iter().any(|c| c.content.contains("Calculator"));
    assert!(has_class, "Should have extracted the Calculator class");
}

#[test]
fn chunk_c_fixture() {
    let source =
        fs::read_to_string("tests/fixtures/sample_c/geometry.c").expect("Failed to read C fixture");
    let chunks = chunk_file(&source, "geometry.c", SupportedLanguage::C);

    assert!(!chunks.is_empty(), "Should produce chunks from C fixture");
    assert!(
        chunks.len() >= 2,
        "Should produce at least 2 chunks (struct + functions), got {}",
        chunks.len()
    );

    for chunk in &chunks {
        assert_eq!(chunk.language, "c");
        assert_eq!(chunk.file_path, "geometry.c");
        assert!(chunk.start_line > 0);
        assert!(!chunk.content.is_empty());
    }
}

#[test]
fn chunk_cpp_fixture() {
    let source = fs::read_to_string("tests/fixtures/sample_cpp/processor.cpp")
        .expect("Failed to read C++ fixture");
    let chunks = chunk_file(&source, "processor.cpp", SupportedLanguage::Cpp);

    assert!(!chunks.is_empty(), "Should produce chunks from C++ fixture");
    assert!(
        chunks.len() >= 2,
        "Should produce at least 2 chunks (classes/namespaces), got {}",
        chunks.len()
    );

    for chunk in &chunks {
        assert_eq!(chunk.language, "cpp");
        assert_eq!(chunk.file_path, "processor.cpp");
        assert!(chunk.start_line > 0);
        assert!(!chunk.content.is_empty());
    }
}

#[test]
fn chunk_ruby_fixture() {
    let source = fs::read_to_string("tests/fixtures/sample_rb/calculator.rb")
        .expect("Failed to read Ruby fixture");
    let chunks = chunk_file(&source, "calculator.rb", SupportedLanguage::Ruby);

    assert!(
        !chunks.is_empty(),
        "Should produce chunks from Ruby fixture"
    );

    for chunk in &chunks {
        assert_eq!(chunk.language, "ruby");
        assert_eq!(chunk.file_path, "calculator.rb");
        assert!(chunk.start_line > 0);
        assert!(!chunk.content.is_empty());
    }

    let has_module = chunks.iter().any(|c| c.content.contains("Calculator"));
    assert!(has_module, "Should have extracted the Calculator module");
}

#[test]
fn chunk_swift_fixture() {
    let source = fs::read_to_string("tests/fixtures/sample_swift/calculator.swift")
        .expect("Failed to read Swift fixture");
    let chunks = chunk_file(&source, "calculator.swift", SupportedLanguage::Swift);

    assert!(
        !chunks.is_empty(),
        "Should produce chunks from Swift fixture"
    );
    assert!(
        chunks.len() >= 2,
        "Should produce at least 2 chunks (class + protocol/enum/struct), got {}",
        chunks.len()
    );

    for chunk in &chunks {
        assert_eq!(chunk.language, "swift");
        assert_eq!(chunk.file_path, "calculator.swift");
        assert!(chunk.start_line > 0);
        assert!(!chunk.content.is_empty());
    }
}

#[test]
fn chunk_kotlin_fixture() {
    let source = fs::read_to_string("tests/fixtures/sample_kt/calculator.kt")
        .expect("Failed to read Kotlin fixture");
    let chunks = chunk_file(&source, "calculator.kt", SupportedLanguage::Kotlin);

    assert!(
        !chunks.is_empty(),
        "Should produce chunks from Kotlin fixture"
    );
    assert!(
        chunks.len() >= 2,
        "Should produce at least 2 chunks (class + interface/object/enum), got {}",
        chunks.len()
    );

    for chunk in &chunks {
        assert_eq!(chunk.language, "kotlin");
        assert_eq!(chunk.file_path, "calculator.kt");
        assert!(chunk.start_line > 0);
        assert!(!chunk.content.is_empty());
    }
}
