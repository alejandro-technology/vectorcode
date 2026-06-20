//! Integration tests for the MCP server using rmcp SDK.

use assert_cmd::prelude::*;
use std::io::Write;
use std::process::{Command, Stdio};

/// Helper: spawn the vectorcode binary with `serve --mcp` and return the child process.
fn spawn_mcp_server(project_path: &std::path::Path) -> std::process::Child {
    Command::cargo_bin("vectorcode")
        .unwrap()
        .arg("serve")
        .arg("--mcp")
        .arg("--project-path")
        .arg(project_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to spawn vectorcode serve --mcp")
}

/// Helper: send a JSON-RPC message to the server's stdin and read one line from stdout.
fn send_and_receive(child: &mut std::process::Child, message: &str) -> String {
    use std::io::{BufRead, BufReader};

    let stdin = child.stdin.as_mut().unwrap();
    writeln!(stdin, "{message}").unwrap();
    stdin.flush().unwrap();

    let stdout = child.stdout.as_mut().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut response = String::new();
    reader.read_line(&mut response).unwrap();
    response.trim().to_string()
}

/// Helper: send a JSON-RPC notification (no response expected).
fn send_notification(child: &mut std::process::Child, message: &str) {
    let stdin = child.stdin.as_mut().unwrap();
    writeln!(stdin, "{message}").unwrap();
    stdin.flush().unwrap();
}

/// Helper: perform proper initialization.
fn initialize_mcp(child: &mut std::process::Child, dir: &std::path::Path) {
    let req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{"roots":{"listChanged":true}},"clientInfo":{"name":"test","version":"1.0"}}}"#;
    let _resp = send_and_receive(child, req);
    let notif = r#"{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}"#;
    send_notification(child, notif);

    // The server now requests roots/list on initialization. Let's read it and reply!
    use std::io::{BufRead, BufReader};
    let stdout = child.stdout.as_mut().unwrap();
    let mut reader = BufReader::new(stdout);
    let mut request_str = String::new();
    reader.read_line(&mut request_str).unwrap();
    eprintln!("READ: {}", request_str);

    // Parse the request to get the ID
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&request_str) {
        if parsed["method"] == "roots/list" {
            let id = parsed["id"].as_i64().unwrap();
            let uri = format!("file://{}", dir.display());
            let reply = format!(
                r#"{{"jsonrpc":"2.0","id":{},"result":{{"roots":[{{"uri":"{}"}}]}}}}"#,
                id, uri
            );
            send_notification(child, &reply);
        }
    }
}

/// Helper: initialize a vectorcode project in the given directory with mock provider.
fn init_project(dir: &std::path::Path) {
    let vc_dir = dir.join(".vectorcode");
    std::fs::create_dir_all(&vc_dir).unwrap();
    std::fs::write(vc_dir.join("config.toml"), "[provider]\nname = \"mock\"\n").unwrap();
    std::fs::write(vc_dir.join(".gitignore"), "index.db\n").unwrap();

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
        files_indexed: 42,
        chunks_stored: 200,
        vectorcode_version: "0.1.0".to_string(),
    };
    vectorcode::store::meta::write_index_meta(file_db.conn(), &meta).unwrap();
}

#[test]
fn mcp_initialize_returns_server_info() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    let mut child = spawn_mcp_server(dir.path());

    let request = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;
    let response = send_and_receive(&mut child, request);

    let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["id"], 1);
    assert_eq!(parsed["result"]["protocolVersion"], "2024-11-05");

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_tools_list_returns_eight_tools() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    let mut child = spawn_mcp_server(dir.path());
    initialize_mcp(&mut child, dir.path());

    let request = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
    let response = send_and_receive(&mut child, request);

    let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["id"], 2);

    let tools = parsed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 8);

    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"vec_search"));
    assert!(names.contains(&"vec_status"));
    assert!(names.contains(&"vec_reindex"));
    assert!(names.contains(&"vec_read_lines"));
    assert!(names.contains(&"vec_outline"));
    assert!(names.contains(&"vec_find_callers"));
    assert!(names.contains(&"vec_find_dependents"));
    assert!(names.contains(&"vec_trace_imports"));

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_vec_status_returns_index_info() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    let mut child = spawn_mcp_server(dir.path());
    initialize_mcp(&mut child, dir.path());

    let request = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"vec_status","arguments":{}}}"#;
    let response = send_and_receive(&mut child, request);

    let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(parsed["id"], 3);

    let content = parsed["result"]["content"].as_array().unwrap();
    assert!(!content.is_empty());
    assert_eq!(content[0]["type"], "text");

    let text = content[0]["text"].as_str().unwrap();
    assert!(text.contains("VectorCode Index Status"));
    assert!(text.contains("Provider:    mock"));

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_initialize_instructions_warn_against_sequential_reads() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    let mut child = spawn_mcp_server(dir.path());

    let request = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;
    let response = send_and_receive(&mut child, request);

    let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
    let instructions = parsed["result"]["instructions"]
        .as_str()
        .expect("instructions should be present");
    assert!(
        instructions.contains("vec_read_lines") && instructions.contains("sequentially"),
        "Server instructions should warn against sequential vec_read_lines calls"
    );

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_closes_cleanly_on_stdin_eof() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    let mut child = spawn_mcp_server(dir.path());

    let request = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#;
    let response = send_and_receive(&mut child, request);
    assert!(response.contains("2024-11-05"));

    child.stdin.take().unwrap();
    let _status = child.wait().unwrap();
}

/// Helper: call an MCP tool and return the parsed response.
fn call_mcp_tool(
    child: &mut std::process::Child,
    tool_name: &str,
    arguments: &str,
    id: i64,
) -> serde_json::Value {
    let request = format!(
        r#"{{"jsonrpc":"2.0","id":{},"method":"tools/call","params":{{"name":"{}","arguments":{}}}}}"#,
        id, tool_name, arguments
    );
    let response = send_and_receive(child, &request);
    serde_json::from_str(&response).unwrap()
}

#[test]
fn mcp_vec_outline_rejects_path_outside_project() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    let mut child = spawn_mcp_server(dir.path());
    initialize_mcp(&mut child, dir.path());

    let parsed = call_mcp_tool(
        &mut child,
        "vec_outline",
        r#"{"file_path":"../../etc/passwd"}"#,
        10,
    );

    // Should have an error in the response
    let content = parsed["result"]["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("not found") || text.contains("Access denied") || text.contains("error"),
        "Should reject path outside project, got: {text}"
    );

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_vec_outline_file_not_found() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    let mut child = spawn_mcp_server(dir.path());
    initialize_mcp(&mut child, dir.path());

    let parsed = call_mcp_tool(
        &mut child,
        "vec_outline",
        r#"{"file_path":"nonexistent_file.rs"}"#,
        11,
    );

    let content = parsed["result"]["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("not found") || text.contains("File not found"),
        "Should return file not found error, got: {text}"
    );

    child.stdin.take().unwrap();
    let _ = child.wait();
}

// ─── Graph tools integration tests ────────────────────────────────────────

/// Helper: insert graph nodes and edges into the project's database.
fn insert_graph_data(dir: &std::path::Path) {
    let db_path = dir.join(".vectorcode").join("index.db");
    let db = vectorcode::Database::open(&db_path).unwrap();

    use vectorcode::types::{EdgeType, GraphEdge, GraphNode};
    use vectorcode::GraphStore;

    let nodes = vec![
        GraphNode {
            id: "main".into(),
            symbol: "main".into(),
            kind: "function".into(),
            file_path: "src/main.rs".into(),
        },
        GraphNode {
            id: "search".into(),
            symbol: "search".into(),
            kind: "function".into(),
            file_path: "src/search.rs".into(),
        },
        GraphNode {
            id: "base".into(),
            symbol: "Base".into(),
            kind: "class".into(),
            file_path: "src/base.rs".into(),
        },
        GraphNode {
            id: "derived".into(),
            symbol: "Derived".into(),
            kind: "class".into(),
            file_path: "src/derived.rs".into(),
        },
        GraphNode {
            id: "module".into(),
            symbol: "my_module".into(),
            kind: "module".into(),
            file_path: "src/module.rs".into(),
        },
        GraphNode {
            id: "serde".into(),
            symbol: "serde".into(),
            kind: "external".into(),
            file_path: "".into(),
        },
    ];

    let edges = vec![
        // main calls search
        GraphEdge {
            source_id: "main".into(),
            target_symbol: "search".into(),
            edge_type: EdgeType::Call,
        },
        // Derived extends Base
        GraphEdge {
            source_id: "derived".into(),
            target_symbol: "Base".into(),
            edge_type: EdgeType::Extends,
        },
        // my_module imports serde
        GraphEdge {
            source_id: "module".into(),
            target_symbol: "serde".into(),
            edge_type: EdgeType::Import,
        },
    ];

    db.insert_nodes(&nodes).unwrap();
    db.insert_edges(&edges).unwrap();
}

#[test]
fn mcp_tools_list_includes_graph_tools() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    let mut child = spawn_mcp_server(dir.path());
    initialize_mcp(&mut child, dir.path());

    let request = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
    let response = send_and_receive(&mut child, request);

    let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
    let tools = parsed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 8, "Should have 8 tools (5 original + 3 graph)");

    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"vec_find_callers"));
    assert!(names.contains(&"vec_find_dependents"));
    assert!(names.contains(&"vec_trace_imports"));

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_vec_find_callers_returns_callers() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    insert_graph_data(dir.path());
    let mut child = spawn_mcp_server(dir.path());
    initialize_mcp(&mut child, dir.path());

    let parsed = call_mcp_tool(&mut child, "vec_find_callers", r#"{"symbol":"search"}"#, 20);

    let content = parsed["result"]["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("main"),
        "Should find main as caller of search, got: {text}"
    );
    assert!(
        text.contains("Found"),
        "Should have 'Found' header, got: {text}"
    );

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_vec_find_dependents_returns_dependents() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    insert_graph_data(dir.path());
    let mut child = spawn_mcp_server(dir.path());
    initialize_mcp(&mut child, dir.path());

    let parsed = call_mcp_tool(
        &mut child,
        "vec_find_dependents",
        r#"{"symbol":"Base"}"#,
        21,
    );

    let content = parsed["result"]["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("Derived"),
        "Should find Derived as dependent of Base, got: {text}"
    );

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_vec_trace_imports_returns_imports() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    insert_graph_data(dir.path());
    let mut child = spawn_mcp_server(dir.path());
    initialize_mcp(&mut child, dir.path());

    let parsed = call_mcp_tool(
        &mut child,
        "vec_trace_imports",
        r#"{"symbol":"my_module"}"#,
        22,
    );

    let content = parsed["result"]["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("serde"),
        "Should find serde as import of my_module, got: {text}"
    );

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_vec_find_callers_empty_graph_message() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());
    // No graph data inserted
    let mut child = spawn_mcp_server(dir.path());
    initialize_mcp(&mut child, dir.path());

    let parsed = call_mcp_tool(
        &mut child,
        "vec_find_callers",
        r#"{"symbol":"nonexistent"}"#,
        23,
    );

    let content = parsed["result"]["content"].as_array().unwrap();
    let text = content[0]["text"].as_str().unwrap();
    assert!(
        text.contains("No graph data") || text.contains("reindex"),
        "Should return empty graph message, got: {text}"
    );

    child.stdin.take().unwrap();
    let _ = child.wait();
}
