//! Record-independent discovery and compatibility names for optional applications.
use serde_json::{Value, json};
use std::ffi::OsString;

pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub code: i32,
}

#[derive(Default)]
pub struct Routing {
    pub early: Option<Output>,
    pub requested: Option<String>,
    pub notice: Option<String>,
}

pub fn catalog() -> Value {
    json!({"applications":[
        {"name":"hub","layer":"application","status":"experimental","extra":"html",
         "command":"kpop experimental hub","aliases":["page"],"display_name":"kpopper Hub",
         "description":"Render or verify the record's HTML page.",
         "usage":"[--open] [--out PATH] [--tree] [--verify] [--checks]"},
        {"name":"annotated-doc","layer":"application","status":"experimental","extra":"html",
         "command":"kpop experimental annotated-doc","aliases":["document"],"display_name":"Annotated Documents",
         "description":"Author and refresh standalone HTML with evidence.","usage":"guide|build|inspect|refresh [OPTIONS]"}
    ]})
}

fn error(message: String) -> Routing {
    Routing {
        early: Some(Output {
            stdout: String::new(),
            stderr: format!(
                "usage: kpop [--workspace PATH] [--no-cache] COMMAND [OPTIONS]\nkpop: error: {message}\n"
            ),
            code: 2,
        }),
        ..Routing::default()
    }
}

/// Inspect only known root flags. Invalid or unfamiliar syntax stays with clap.
pub fn inspect(argv: &[OsString]) -> Routing {
    let mut at = 1;
    let mut as_json = false;
    while let Some(arg) = argv.get(at).and_then(|s| s.to_str()) {
        match arg {
            "--json" => as_json = true,
            "--frozen" | "--no-cache" => {}
            "--workspace" => {
                at += 1;
                if argv.get(at).is_none() {
                    return Routing::default();
                }
            }
            s if s.starts_with("--workspace=") => {}
            s if s.starts_with('-') => return Routing::default(),
            _ => break,
        }
        at += 1;
    }
    let Some(command) = argv.get(at).and_then(|s| s.to_str()) else {
        return Routing::default();
    };
    let explicit = command == "experimental";
    let requested = if explicit {
        at += 1;
        while argv.get(at).is_some_and(|s| s == "--json") {
            as_json = true;
            at += 1;
        }
        let rest = &argv[at..];
        if rest
            .iter()
            .all(|s| ["-h", "--help", "--json"].iter().any(|t| s == t))
        {
            as_json |= rest.iter().any(|s| s == "--json");
            let stdout = if as_json {
                format!("{}\n", catalog())
            } else {
                // Rendering is included in the native package, so no Python
                // installation commands are advertised by this catalog.
                "usage: kpop experimental APPLICATION [OPTIONS]\n\nExperimental applications:\n  kpop experimental hub          Render or verify the record's HTML page.\n  kpop experimental annotated-doc Author and refresh standalone HTML with evidence.\n\nInterfaces and artifact formats may change. Use APPLICATION --help for details.\n".into()
            };
            return Routing {
                early: Some(Output {
                    stdout,
                    stderr: String::new(),
                    code: 0,
                }),
                ..Routing::default()
            };
        }
        let Some(name) = argv.get(at).and_then(|s| s.to_str()) else {
            return Routing::default();
        };
        if !["hub", "annotated-doc", "page", "document"].contains(&name) {
            return error(format!("unknown experimental application: {name}"));
        }
        name
    } else {
        if ["hub", "annotated-doc"].contains(&command) {
            return error(format!(
                "use kpop experimental {command} for this application"
            ));
        }
        command
    };
    let canonical = match requested {
        "page" => "hub",
        "document" => "annotated-doc",
        "hub" | "annotated-doc" => requested,
        _ => return Routing::default(),
    };
    let notice=(requested!=canonical).then(||format!("kpop {}{requested} is a compatibility alias; use kpop experimental {canonical} (experimental application).\n",if explicit{"experimental "}else{""}));
    Routing {
        early: None,
        requested: Some(requested.into()),
        notice,
    }
}
