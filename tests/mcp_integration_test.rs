//! Integration tests for the MCP server — spawn binary, send JSON-RPC, verify responses.

use std::io::Write;
use std::process::{Command, Stdio};

use assert_cmd::prelude::*;

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

/// Helper: initialize a vectorcode project in the given directory with mock provider.
fn init_project(dir: &std::path::Path) {
    // Create .vectorcode directory structure
    let vc_dir = dir.join(".vectorcode");
    std::fs::create_dir_all(&vc_dir).unwrap();

    // Create config.toml with mock provider
    std::fs::write(
        vc_dir.join("config.toml"),
        r#"
[provider]
name = "mock"
"#,
    )
    .unwrap();

    // Create .gitignore
    std::fs::write(vc_dir.join(".gitignore"), "index.db\n").unwrap();

    // Create the database
    let db = vectorcode::Database::open_in_memory().unwrap();
    db.init_schema(384).unwrap();

    // Copy the in-memory DB to a file (we need a real file for the server)
    let db_path = vc_dir.join("index.db");
    let file_db = vectorcode::Database::open(&db_path).unwrap();
    file_db.init_schema(384).unwrap();

    // Write meta
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

    let request = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let response = send_and_receive(&mut child, request);

    // Parse response
    let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["id"], 1);
    assert_eq!(parsed["result"]["serverInfo"]["name"], "vectorcode");
    assert_eq!(parsed["result"]["serverInfo"]["version"], "0.1.0");
    assert_eq!(parsed["result"]["protocolVersion"], "2024-11-05");
    assert!(parsed["result"]["capabilities"]["tools"].is_object());

    // Clean shutdown
    child.stdin.take().unwrap(); // Close stdin
    let _ = child.wait();
}

#[test]
fn mcp_tools_list_returns_three_tools() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());

    let mut child = spawn_mcp_server(dir.path());

    let request = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
    let response = send_and_receive(&mut child, request);

    let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(parsed["jsonrpc"], "2.0");
    assert_eq!(parsed["id"], 2);

    let tools = parsed["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 3);

    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"vec_search"));
    assert!(names.contains(&"vec_status"));
    assert!(names.contains(&"vec_reindex"));

    // Verify vec_search has required input schema
    let vec_search = tools.iter().find(|t| t["name"] == "vec_search").unwrap();
    assert!(vec_search["inputSchema"]["properties"]["query"].is_object());
    assert!(vec_search["inputSchema"]["required"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == "query"));

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_vec_status_returns_index_info() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());

    let mut child = spawn_mcp_server(dir.path());

    // First initialize
    let init_req = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let _ = send_and_receive(&mut child, init_req);

    // Then call vec_status
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
    assert!(text.contains("Model:       mock-embedder"));
    assert!(text.contains("Dimensions:  384"));
    assert!(text.contains("Files:       42 indexed"));
    assert!(text.contains("Chunks:      200 stored"));

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_unknown_method_returns_error() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());

    let mut child = spawn_mcp_server(dir.path());

    let request = r#"{"jsonrpc":"2.0","id":99,"method":"nonexistent/method","params":{}}"#;
    let response = send_and_receive(&mut child, request);

    let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(parsed["id"], 99);
    assert!(parsed["error"].is_object());
    assert_eq!(parsed["error"]["code"], -32601);
    assert!(parsed["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Method not found"));

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_invalid_json_returns_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());

    let mut child = spawn_mcp_server(dir.path());

    let request = "this is not valid json";
    let response = send_and_receive(&mut child, request);

    let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert!(parsed["error"].is_object());
    assert_eq!(parsed["error"]["code"], -32700);
    assert!(parsed["error"]["message"]
        .as_str()
        .unwrap()
        .contains("Parse error"));

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_unknown_tool_returns_error_result() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());

    let mut child = spawn_mcp_server(dir.path());

    let request = r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"nonexistent_tool","arguments":{}}}"#;
    let response = send_and_receive(&mut child, request);

    let parsed: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(parsed["id"], 5);

    let content = parsed["result"]["content"].as_array().unwrap();
    assert!(!content.is_empty());
    let text = content[0]["text"].as_str().unwrap();
    assert!(text.contains("Unknown tool"));
    assert_eq!(parsed["result"]["isError"], true);

    child.stdin.take().unwrap();
    let _ = child.wait();
}

#[test]
fn mcp_closes_cleanly_on_stdin_eof() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());

    let mut child = spawn_mcp_server(dir.path());

    // Send one message to verify server is running
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let response = send_and_receive(&mut child, request);
    assert!(response.contains("vectorcode"));

    // Close stdin — server should exit cleanly
    child.stdin.take().unwrap();
    let status = child.wait().unwrap();
    assert!(status.success(), "Server should exit cleanly on stdin EOF");
}

#[test]
fn mcp_multiple_requests_in_sequence() {
    let dir = tempfile::tempdir().unwrap();
    init_project(dir.path());

    let mut child = spawn_mcp_server(dir.path());

    // Send initialize
    let req1 = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}"#;
    let resp1 = send_and_receive(&mut child, req1);
    let parsed1: serde_json::Value = serde_json::from_str(&resp1).unwrap();
    assert_eq!(parsed1["id"], 1);
    assert_eq!(parsed1["result"]["serverInfo"]["name"], "vectorcode");

    // Send tools/list
    let req2 = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#;
    let resp2 = send_and_receive(&mut child, req2);
    let parsed2: serde_json::Value = serde_json::from_str(&resp2).unwrap();
    assert_eq!(parsed2["id"], 2);
    assert!(parsed2["result"]["tools"].as_array().unwrap().len() == 3);

    // Send vec_status
    let req3 = r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"vec_status","arguments":{}}}"#;
    let resp3 = send_and_receive(&mut child, req3);
    let parsed3: serde_json::Value = serde_json::from_str(&resp3).unwrap();
    assert_eq!(parsed3["id"], 3);
    assert!(parsed3["result"]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("VectorCode Index Status"));

    child.stdin.take().unwrap();
    let _ = child.wait();
}
