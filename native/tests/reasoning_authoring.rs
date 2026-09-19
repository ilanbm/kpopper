use kpop_native::{
    Error, Result,
    reasoning_authoring::{self as A, World},
    reasoning_runtime::{OperationalBounds, Runtime, target_name},
    reasoning_snapshot::{CaptureOptions, Snapshot},
    value::TypedValue as V,
};
use serde_json::Value as J;
fn normalize(v: &mut V) {
    match v {
        V::Map(m) => {
            m.remove("assessment_revision");
            if matches!(m.get("implementation"), Some(V::Map(_))) {
                m.insert("implementation".into(), V::Null);
            }
            if let Some(V::Map(a)) = m.get_mut("assurance") {
                a.remove("implementation");
            }
            for v in m.values_mut() {
                normalize(v)
            }
        }
        V::List(a) => {
            for v in a {
                normalize(v)
            }
        }
        _ => {}
    }
}
fn diagnostic_commands(value: &mut V) {
    let V::List(lines) = value else { return };
    for line in lines {
        let V::Text(line) = line else { continue };
        let marker = [": add ", ": set "]
            .into_iter()
            .filter_map(|p| line.rfind(p).map(|i| i + 2))
            .max();
        let Some(start) = marker else { continue };
        let words =
            shlex::split(&line[start..]).expect("refusal suggestion must be valid shell syntax");
        let mut fields = std::collections::BTreeMap::new();
        let mut options = Vec::new();
        let mut positional = Vec::new();
        let mut i = 2;
        while i < words.len() {
            if words[i].starts_with("--") {
                assert!(i + 1 < words.len());
                options.push((words[i].clone(), words[i + 1].clone()));
                i += 2;
            } else if words[0] == "add" && words[i].contains('=') {
                let (key, v) = words[i].split_once('=').unwrap();
                let parsed =
                    kpop_native::history_yaml::decode_document(format!("value: {v}\n").as_bytes());
                fields.insert(
                    key.to_owned(),
                    parsed
                        .map(|v| v.to_tagged().unwrap())
                        .unwrap_or_else(|_| serde_json::json!(v)),
                );
                i += 1;
            } else {
                positional.push(words[i].clone());
                i += 1;
            }
        }
        options.sort();
        *line = format!(
            "{}{}",
            &line[..start],
            serde_json::json!({"kind":words[0],"id":words[1],"fields":fields,"options":options,"positional":positional})
        );
    }
}

fn run(c: &J, runtime: &Runtime) -> Result<V> {
    let doc = V::from_tagged(&c["document"])?;
    let arg = V::from_tagged(&c["argument"])?;
    if c["op"] == "declaration" {
        return A::declaration(&doc);
    }
    if c["op"] == "selected" {
        return Ok(V::Bool(A::selected(
            &doc,
            match &arg {
                V::Text(s) => Some(s),
                _ => None,
            },
        )?));
    }
    let snapshot = Snapshot::from_data(
        &doc,
        CaptureOptions {
            as_of: Some(V::Text("2026-09-19".into())),
            hypotheses: c.get("hypotheses").map(V::from_tagged).transpose()?,
            ..Default::default()
        },
    )?;
    let mut w = World::new(
        &doc,
        Some(&snapshot),
        Some(runtime),
        OperationalBounds::default(),
    )?;
    let str_arg = if let V::Text(s) = &arg {
        s.as_str()
    } else {
        ""
    };
    match c["op"].as_str().unwrap() {
        "declaration" => A::declaration(&doc),
        "selected" => Ok(V::Bool(A::selected(
            &doc,
            if arg == V::Null { None } else { Some(str_arg) },
        )?)),
        "result" => V::from_json(&w.result(str_arg)?),
        "value" => w.value(str_arg),
        "history" => w.history(str_arg),
        "state" => w.state(str_arg),
        "predicate" => Ok(w.predicate(&arg, None)?.map(V::Bool).unwrap_or(V::Null)),
        "candidate" => Ok(w.candidate(&arg)?.document().clone()),
        "normalize" => {
            let (a, n) = w.normalize(&arg, None)?;
            Ok(V::List(vec![
                a,
                V::List(n.into_iter().map(V::Text).collect()),
            ]))
        }
        "validate" => Ok(V::List(
            w.validate(&arg)?.into_iter().map(V::Text).collect(),
        )),
        "same_value" => {
            let V::List(a) = arg else { panic!() };
            let V::Text(id) = &a[0] else { panic!() };
            Ok(V::Bool(w.same_value(id, &a[1])?))
        }
        _ => Err(Error("unknown test operation".into())),
    }
}
#[test]
fn writer_values_history_normalization_and_admission_match_pinned_python() {
    let archive = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/reasoning/native")
        .join(format!("{}.zip", target_name().unwrap()));
    let cache = tempfile::tempdir().unwrap();
    let runtime = Runtime::open(&archive, cache.path(), OperationalBounds::default()).unwrap();
    let corpus: J =
        serde_json::from_str(include_str!("fixtures/reasoning-authoring.json")).unwrap();
    let mut failures = vec![];
    for c in corpus["cases"].as_array().unwrap() {
        match run(c, &runtime) {
            Ok(mut output) => {
                if let Some(expected) = c.get("output") {
                    let mut expected = V::from_tagged(expected).unwrap();
                    normalize(&mut output);
                    normalize(&mut expected);
                    if c["op"] == "validate" {
                        diagnostic_commands(&mut output);
                        diagnostic_commands(&mut expected);
                    }
                    // Parser diagnostics identify the same refusal, but the Rust parser
                    // does not claim to emit CPython exception wording.
                    if c["op"] == "normalize" {
                        for v in [&mut output, &mut expected] {
                            if let V::List(parts) = v
                                && let V::List(notes) = &mut parts[1]
                            {
                                for note in notes {
                                    if let V::Text(t) = note
                                        && (t.contains("invalid expression syntax")
                                            || t.contains("(<unknown>, line 1)"))
                                    {
                                        *t = t.split(": ").next().unwrap().to_owned()
                                            + ": syntax error";
                                    }
                                }
                            }
                        }
                    }
                    if output != expected {
                        failures.push(format!(
                            "{} actual={} expected={}",
                            c["name"],
                            serde_json::to_string(&output.to_tagged().unwrap()).unwrap(),
                            serde_json::to_string(&expected.to_tagged().unwrap()).unwrap()
                        ));
                    }
                } else {
                    failures.push(format!(
                        "{} accepted; expected refusal {}",
                        c["name"], c["refused"]
                    ))
                }
            }
            Err(e) => {
                if c.get("refused").is_none() {
                    failures.push(format!("{} refused: {e}", c["name"]));
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
