//! Small, bounded JSON-RPC transport for the session MCP service.
//!
//! This module deliberately contains no session, filesystem, or model logic.  The
//! caller supplies the declarations it implements and a handler for tool calls.

use std::io::{BufRead, Write};

use serde_json::{Value, json};

pub const MAX_LINE_BYTES: usize = 1_048_576;
pub const SUPPORTED_PROTOCOL_VERSIONS: [&str; 3] = ["2024-11-05", "2025-03-26", "2025-06-18"];

#[derive(Clone, Debug)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub annotations: Value,
}

impl Tool {
    pub fn new(name: impl Into<String>, description: impl Into<String>, input_schema: Value) -> Self {
        Self { name: name.into(), description: description.into(), input_schema, annotations: json!({}) }
    }
    pub fn with_annotations(mut self, annotations: Value) -> Self { self.annotations = annotations; self }
}

/// Declarations corresponding to the reference Python MCP service. Callers should
/// pass only the entries whose implementation they actually provide to [`server`].
pub fn declared_tools() -> Vec<Tool> {
    vec![
        Tool::new("kpopper_open", "Open the bound project's checked map within a reference-token budget.", json!({"type":"object","properties":{"tokens":{"type":"integer","default":700}},"additionalProperties":false})).with_annotations(json!({"readOnlyHint":true})),
        Tool::new("kpopper_read", "Read a returned reference at its revision; exact text supports labeled chunks.", json!({"type":"object","properties":{"ref":{"type":"string"},"revision":{"type":"string"},"tokens":{"type":"integer","default":1600},"offset":{"type":["integer","null"]}},"required":["ref","revision"],"additionalProperties":false})).with_annotations(json!({"readOnlyHint":true})),
        Tool::new("kpopper_search", "Find candidate refs; known IDs win and branches never exclude stronger matches.", json!({"type":"object","properties":{"query":{"type":"string"},"revision":{"type":"string"},"tokens":{"type":"integer","default":1000},"ids":{"type":["array","null"],"items":{"type":"string"}},"limit":{"type":"integer","default":8},"branch":{"type":["string","null"]},"mode":{"type":"string","enum":["lexical","semantic","hybrid"],"default":"hybrid"},"cursor":{"type":["string","null"]}},"required":["query","revision"],"additionalProperties":false})).with_annotations(json!({"readOnlyHint":true})),
        Tool::new("kpopper_context", "Read exact selected nodes and declared neighbors within a token budget.", json!({"type":"object","properties":{"ids":{"type":"array","items":{"type":"string"}},"revision":{"type":"string"},"direction":{"type":"string","enum":["support","impact"]},"tokens":{"type":"integer","default":2000},"depth":{"type":"integer","default":1},"max_nodes":{"type":"integer","default":16}},"required":["ids","revision","direction"],"additionalProperties":false})).with_annotations(json!({"readOnlyHint":true})),
        Tool::new("kpopper_propose", "Save a pending proposal. This does not change the canonical project record.", json!({"type":"object","properties":{"revision":{"type":"string"},"kind":{"type":"string","enum":["observed","inferred","assumed","question"]},"text":{"type":"string"},"basis":{"type":"array","items":{"type":"string"}},"revisit":{"type":"string","default":""}},"required":["revision","kind","text","basis"],"additionalProperties":false})).with_annotations(json!({"readOnlyHint":false,"destructiveHint":false,"idempotentHint":true})),
        Tool::new("kpopper_verify_claims", "Check listed structured assertions only, not accompanying prose or world truth.", json!({"type":"object","properties":{"judgment":{"type":"string"},"revision":{"type":"string"},"assertions":{"type":"array","items":{"type":"object"}}},"required":["judgment","revision","assertions"],"additionalProperties":false})).with_annotations(json!({"readOnlyHint":true})),
    ]
}

#[derive(Debug)]
pub enum Error {
    Io(std::io::Error),
    LineTooLong,
    OutputTooLong,
}

impl From<std::io::Error> for Error { fn from(e: std::io::Error) -> Self { Self::Io(e) } }

type RpcId = Value;
type RequestParts<'a> = Result<(Option<RpcId>, String, &'a Value), (Option<RpcId>, &'static str)>;

/// Serve newline-delimited JSON-RPC requests until EOF.
pub fn server<R, W, F>(reader: &mut R, writer: &mut W, tools: &[Tool], mut handler: F) -> Result<(), Error>
where
    R: BufRead,
    W: Write,
    F: FnMut(&str, &Value) -> Result<String, String>,
{
    let mut line = Vec::new();
    loop {
        line.clear();
        let mut byte = [0_u8; 1];
        loop {
            let read = reader.read(&mut byte)?;
            if read == 0 { if line.is_empty() { return Ok(()); } break; }
            line.push(byte[0]);
            if byte[0] == b'\n' { break; }
            if line.len() > MAX_LINE_BYTES { return Err(Error::LineTooLong); }
        }
        while line.last().is_some_and(|b| *b == b'\n' || *b == b'\r') { line.pop(); }
        if line.is_empty() { continue; }
        let value: Value = match serde_json::from_slice(&line) {
            Ok(v) => v,
            Err(_) => { write_error(writer, None, -32700, "Parse error")?; continue; }
        };
        let (id, method, params) = match request_parts(&value) {
            Ok(parts) => parts,
            Err((id, message)) => { if id.is_some() { write_error(writer, id, -32600, message)?; } continue; }
        };
        let Some(id) = id else {
            // Notifications, including initialized/cancelled, never receive a response.
            continue;
        };
        let response = dispatch(&method, params, &id, tools, &mut handler);
        write_json(writer, &response)?;
    }
}

fn request_parts(value: &Value) -> RequestParts<'_> {
    let object = value.as_object().ok_or((None, "Invalid Request"))?;
    if object.get("jsonrpc") != Some(&Value::String("2.0".into())) { return Err((object.get("id").cloned(), "Invalid Request")); }
    let method = object.get("method").and_then(Value::as_str).ok_or((object.get("id").cloned(), "Invalid Request"))?;
    let id = match object.get("id") {
        None => None,
        Some(Value::String(_) | Value::Number(_)) => Some(object["id"].clone()),
        Some(_) => return Err((None, "Invalid Request")),
    };
    Ok((id, method.to_owned(), object.get("params").unwrap_or(&Value::Null)))
}

fn dispatch<F>(method: &str, params: &Value, id: &RpcId, tools: &[Tool], handler: &mut F) -> Value
where F: FnMut(&str, &Value) -> Result<String, String> {
    let result = match method {
        "initialize" => initialize(params),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(json!({ "tools": tools.iter().map(tool_json).collect::<Vec<_>>() })),
        "tools/call" => call_tool(params, tools, handler),
        _ => Err((-32601, "Method not found".into(), false)),
    };
    match result {
        Ok(value) => json!({"jsonrpc":"2.0", "id":id, "result":value}),
        Err((_code, message, application)) if application => json!({"jsonrpc":"2.0", "id":id, "result":{"content":[{"type":"text","text":message}],"isError":true}}),
        Err((code, message, _)) => json!({"jsonrpc":"2.0", "id":id, "error":{"code":code,"message":message}}),
    }
}

fn initialize(params: &Value) -> Result<Value, (i32, String, bool)> {
    let requested = params.get("protocolVersion").and_then(Value::as_str).ok_or((-32602, "Invalid params".into(), false))?;
    let version = SUPPORTED_PROTOCOL_VERSIONS.iter().find(|v| **v == requested).copied().unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[2]);
    Ok(json!({"protocolVersion":version,"capabilities":{"tools":{}},"serverInfo":{"name":"kpopper","version":"0.1.0"}}))
}

fn tool_json(tool: &Tool) -> Value {
    json!({"name":tool.name,"description":tool.description,"inputSchema":tool.input_schema,"annotations":tool.annotations})
}

fn call_tool<F>(params: &Value, tools: &[Tool], handler: &mut F) -> Result<Value, (i32, String, bool)>
where F: FnMut(&str, &Value) -> Result<String, String> {
    let object = params.as_object().ok_or((-32602, "Invalid params".into(), false))?;
    let name = object.get("name").and_then(Value::as_str).ok_or((-32602, "Invalid params".into(), false))?;
    let empty_arguments = Value::Object(Default::default());
    let arguments = object.get("arguments").unwrap_or(&empty_arguments);
    let tool = tools.iter().find(|t| t.name == name).ok_or((-32602, "Unknown tool".into(), false))?;
    validate_schema(arguments, &tool.input_schema).map_err(|e| (-32602, e, false))?;
    match handler(name, arguments) {
        Ok(text) => Ok(json!({"content":[{"type":"text","text":text}],"isError":false})),
        Err(message) => Err((0, message, true)),
    }
}

fn validate_schema(value: &Value, schema: &Value) -> Result<(), String> {
    if let Some(required) = schema.get("required").and_then(Value::as_array) {
        let object = value.as_object().ok_or_else(|| "arguments must be an object".to_owned())?;
        for key in required { let key = key.as_str().ok_or_else(|| "invalid schema".to_owned())?; if !object.contains_key(key) { return Err(format!("missing required argument: {key}")); } }
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        let object = value.as_object().ok_or_else(|| "arguments must be an object".to_owned())?;
        for (key, item) in object { if let Some(rule) = properties.get(key) { validate_type(item, rule, key)?; } else if schema.get("additionalProperties") == Some(&Value::Bool(false)) { return Err(format!("unknown argument: {key}")); } }
    }
    Ok(())
}

fn validate_type(value: &Value, schema: &Value, key: &str) -> Result<(), String> {
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        if options.iter().any(|option| validate_type(value, option, key).is_ok()) { return Ok(()); }
        return Err(format!("argument {key} has invalid type"));
    }
    if let Some(values) = schema.get("enum").and_then(Value::as_array)
        && !values.iter().any(|candidate| candidate == value) { return Err(format!("argument {key} has invalid value")); }
    let Some(kind) = schema.get("type").and_then(Value::as_str) else { return Ok(()); };
    let valid = match kind { "string" => value.is_string(), "integer" => value.as_i64().is_some() || value.as_u64().is_some(), "boolean" => value.is_boolean(), "array" => value.is_array(), "object" => value.is_object(), "null" => value.is_null(), _ => true };
    if valid { Ok(()) } else { Err(format!("argument {key} has invalid type")) }
}

fn write_error<W: Write>(writer: &mut W, id: Option<RpcId>, code: i32, message: &str) -> Result<(), Error> { write_json(writer, &json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":message}})) }
fn write_json<W: Write>(writer: &mut W, value: &Value) -> Result<(), Error> { let bytes = serde_json::to_vec(value).map_err(|_| Error::OutputTooLong)?; if bytes.len() + 1 > MAX_LINE_BYTES { return Err(Error::OutputTooLong); } writer.write_all(&bytes)?; writer.write_all(b"\n")?; writer.flush()?; Ok(()) }
