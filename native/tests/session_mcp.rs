use std::io::Cursor;

use kpop_native::session_mcp::{Tool, server};
use serde_json::{Value, json};

fn tool() -> Tool {
    Tool::new("echo", "Echo x", json!({"type":"object","properties":{"x":{"type":"integer"}},"required":["x"],"additionalProperties":false}))
}

#[test]
fn handshake_list_call_preserves_number_id() {
    let input = json!([
        {"jsonrpc":"2.0","id":"init","method":"initialize","params":{"protocolVersion":"2025-06-18"}},
        {"jsonrpc":"2.0","method":"initialized"},
        {"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}},
        {"jsonrpc":"2.0","id":"call","method":"tools/call","params":{"name":"echo","arguments":{"x":3}}}
    ]);
    // The wire format is one request per line; build it explicitly for this stream.
    let wire = input.as_array().unwrap().iter().map(|v| serde_json::to_string(v).unwrap()).collect::<Vec<_>>().join("\n");
    let mut reader = Cursor::new(wire);
    let mut output = Vec::new();
    server(&mut reader, &mut output, &[tool()], |_, args| Ok(args["x"].to_string())).unwrap();
    let responses: Vec<Value> = String::from_utf8(output).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(responses.len(), 3);
    assert_eq!(responses[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(responses[1]["id"], 7);
    assert_eq!(responses[1]["result"]["tools"][0]["name"], "echo");
    assert_eq!(responses[2]["result"]["content"][0]["text"], "3");
}

#[test]
fn invalid_args_and_application_errors_are_separate() {
    let wire = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"echo","arguments":{"x":true}}}"#, "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":{"x":4}}}"#, "\n",
    );
    let mut reader = Cursor::new(wire);
    let mut output = Vec::new();
    server(&mut reader, &mut output, &[tool()], |_, _| Err("application failed".into())).unwrap();
    let responses: Vec<Value> = String::from_utf8(output).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(responses[0]["error"]["code"], -32602);
    assert_eq!(responses[1]["result"]["isError"], true);
}

#[test]
fn malformed_and_notifications_follow_json_rpc_rules() {
    let wire = "not-json\n{\"jsonrpc\":\"2.0\",\"method\":\"ping\"}\n{\"jsonrpc\":\"2.0\",\"id\":\"x\",\"method\":\"missing\"}\n";
    let mut reader = Cursor::new(wire);
    let mut output = Vec::new();
    server(&mut reader, &mut output, &[], |_, _| unreachable!()).unwrap();
    let responses: Vec<Value> = String::from_utf8(output).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
    assert_eq!(responses.len(), 2);
    assert_eq!(responses[0]["error"]["code"], -32700);
    assert_eq!(responses[1]["error"]["code"], -32601);
}
