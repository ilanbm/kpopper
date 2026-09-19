use kpop_native::{
    Result,
    reasoning_basis::InputBasis,
    reasoning_capabilities::{compare_basis, document_capabilities},
    value::TypedValue as V,
};
use std::collections::BTreeMap;
#[test]
fn computational_identity_and_dependent_capabilities_match_python() {
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/reasoning-inputs.json")).unwrap();
    let mut failures = Vec::new();
    for bucket in ["basis", "capabilities", "compare"] {
        for case in corpus[bucket].as_array().unwrap() {
            let input = V::from_tagged(&case["input"]).unwrap();
            let result = (|| -> Result<V> {
                match bucket {
                    "capabilities" => document_capabilities(&input),
                    "compare" => {
                        let V::Map(m) = &input else { panic!() };
                        Ok(V::Text(
                            compare_basis(&m["current"], &m["historical"])?.into(),
                        ))
                    }
                    _ => {
                        let V::Map(m) = &input else { panic!() };
                        let V::List(roots) = &m["roots"] else {
                            panic!()
                        };
                        let roots = roots
                            .iter()
                            .map(|v| {
                                let V::Text(s) = v else { panic!() };
                                s.clone()
                            })
                            .collect::<Vec<_>>();
                        let mut basis = InputBasis::new(&m["snapshot"])?;
                        let summaries = roots
                            .iter()
                            .map(|id| Ok((id.clone(), basis.summary(id)?)))
                            .collect::<Result<BTreeMap<_, _>>>()?;
                        let bases = roots
                            .iter()
                            .map(|id| Ok((id.clone(), basis.basis(id)?)))
                            .collect::<Result<BTreeMap<_, _>>>()?;
                        Ok(V::Map(BTreeMap::from([
                            ("summaries".into(), V::Map(summaries)),
                            ("bases".into(), V::Map(bases)),
                            (
                                "modules".into(),
                                V::List(basis.modules(&roots)?.into_iter().map(V::Text).collect()),
                            ),
                            ("dependencies".into(), basis.dependencies(&roots)?),
                        ])))
                    }
                }
            })();
            match result {
                Ok(v) => {
                    if Some(v.to_tagged().unwrap()) != case.get("output").cloned() {
                        failures.push(format!("{bucket}/{}: accepted mismatch", case["name"]));
                    }
                }
                Err(e) => {
                    if case.get("refused").is_none()
                        && Some(e.0.split(':').next().unwrap()) != case["error"].as_str()
                    {
                        failures.push(format!(
                            "{bucket}/{}: {} expected {}",
                            case["name"], e.0, case["error"]
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
