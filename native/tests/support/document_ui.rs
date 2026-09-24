//! Test-only bridge for the offline DOM suite. The product library exposes no fixture API.
use super::*;

#[test]
#[ignore = "invoked by the DOM fixture generator with explicit request/response paths"]
fn request() {
    let input = std::env::var_os("KPOP_DOCUMENT_REQUEST").expect("DOM request path");
    let output = std::env::var_os("KPOP_DOCUMENT_RESPONSE").expect("DOM response path");
    let request: Value = serde_json::from_slice(&fs::read(input).unwrap()).unwrap();
    let args = &request["args"];
    let result = match request["op"].as_str().unwrap() {
        "build" => build(
            args[0].as_str().unwrap(),
            &args[1],
            Path::new(args[2].as_str().unwrap()),
            args[3].as_str(),
        ),
        "refresh" => refresh(
            &args[0],
            &args[1],
            Path::new(args[2].as_str().unwrap()),
            args[3].as_str(),
        ),
        "render" => render(&args[0]).map(Value::String),
        "load_artifact" => load_artifact(args[0].as_str().unwrap()),
        "summary" => summary(&args[0]),
        "selected_html" => validate_artifact(&args[0])
            .and_then(|()| selected_html(&args[0]))
            .map(Value::String),
        _ => panic!("unknown DOM fixture operation"),
    };
    let result = match result {
        Ok(value) => json!({"ok":value}),
        Err(error) => json!({"error":error.to_string()}),
    };
    fs::write(output, serde_json::to_vec(&result).unwrap()).unwrap();
}
