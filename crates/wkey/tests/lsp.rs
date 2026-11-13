//! Drives `wkey lsp` over stdio like Zed does and reads the cursor file it writes.
use serde_json::{Value, json};
use std::io::{BufReader, Write};
use std::process::{Command, Stdio};

fn frame(m: &Value) -> Vec<u8> {
    let body = m.to_string();
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

#[test]
fn follows_the_cursor_and_exits_cleanly() {
    let runtime = std::env::temp_dir().join(format!("wkey-lsp-{}", std::process::id()));
    let mut child = Command::new(env!("CARGO_BIN_EXE_wkey"))
        .arg("lsp")
        .env("XDG_RUNTIME_DIR", &runtime)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let range = json!({"start": {"line": 2, "character": 5}, "end": {"line": 2, "character": 8}});
    for m in [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "textDocument/codeAction", "params": {
            "textDocument": {"uri": "file:///p/main.rs"}, "range": range, "context": {"diagnostics": []}}}),
    ] {
        stdin.write_all(&frame(&m)).unwrap();
    }
    let init = wkey::lsp::read(&mut stdout).unwrap().unwrap();
    assert_eq!(init["id"], 1);
    let actions = wkey::lsp::read(&mut stdout).unwrap().unwrap();
    assert_eq!(actions, json!({"jsonrpc": "2.0", "id": 2, "result": []}));

    let file = runtime.join("wkey/cursor.json");
    let cursor: Value = serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    assert_eq!(
        cursor,
        json!({"uri": "file:///p/main.rs", "line": 2, "character": 5, "mode": "visual"})
    );

    for m in [
        json!({"jsonrpc": "2.0", "id": 3, "method": "shutdown"}),
        json!({"jsonrpc": "2.0", "method": "exit"}),
    ] {
        stdin.write_all(&frame(&m)).unwrap();
    }
    assert_eq!(wkey::lsp::read(&mut stdout).unwrap().unwrap()["id"], 3);
    assert!(child.wait().unwrap().success());
    std::fs::remove_dir_all(runtime).unwrap();
}
