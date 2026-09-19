//! Public computation envelopes over one immutable Snapshot. Only the verified
//! Lean runtime evaluates expressions; preparation and failed execution stay explicit.
use crate::{
    Error, Result,
    history_contract::map,
    history_yaml,
    reasoning_basis::InputBasis,
    reasoning_fields, reasoning_language as L, reasoning_query as Q, reasoning_query_adapter as QA,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_scope::SnapshotView,
    reasoning_snapshot::{Snapshot, digest},
    require,
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
use std::collections::BTreeSet;
fn hash(v: &J) -> Result<String> {
    digest(&V::from_json_bounded(v, 16 * 1024 * 1024 / 8)?)
}
fn bytes(v: &J) -> Result<usize> {
    Ok(history_yaml::compact_json_size(&V::from_json_bounded(
        v,
        16 * 1024 * 1024 / 8,
    )?))
}
fn diagnostic(code: &str, related: impl IntoIterator<Item = String>) -> J {
    json!({"code":code,"related_ids":related.into_iter().collect::<BTreeSet<_>>()})
}
fn qdiagnostic(code: &str, related: impl IntoIterator<Item = String>) -> J {
    let mut d = diagnostic(code, related);
    d["locations"] = json!([]);
    d
}
fn zero_cost(query: bool) -> J {
    if query {
        json!({"steps":0,"preflight_steps":0,"node_evaluations":{},"candidates":0,"field_reads":0,"evaluated_field_reads":0})
    } else {
        json!({"steps":0,"preflight_steps":0,"node_evaluations":{}})
    }
}
fn cap_error(e: Error) -> Error {
    Error(format!("capability:{}", e.0))
}
fn is_query(v: &V) -> bool {
    matches!(v,V::Map(m)if m.len()==1&&m.contains_key("query"))
}
fn strings(v: &J) -> Result<Vec<String>> {
    v.as_array()
        .ok_or_else(|| Error("invalid references".into()))?
        .iter()
        .map(|v| {
            v.as_str()
                .map(str::to_owned)
                .ok_or_else(|| Error("invalid reference".into()))
        })
        .collect()
}

pub struct Evaluator<'a> {
    snapshot: &'a Snapshot,
    runtime: Option<&'a Runtime>,
    requested: J,
    limits: J,
    bounds: OperationalBounds,
    basis: Option<InputBasis>,
}
impl<'a> Evaluator<'a> {
    pub fn new(
        snapshot: &'a Snapshot,
        runtime: Option<&'a Runtime>,
        limits: Option<J>,
        mut bounds: OperationalBounds,
    ) -> Result<Self> {
        bounds.validate()?;
        require(
            bounds.timeout.subsec_nanos() == 0,
            "invalid operational limit: timeout_seconds",
        )?;
        if let Some(runtime) = runtime {
            let b = runtime.bounds();
            bounds.timeout = bounds.timeout.min(b.timeout);
            bounds.batch_requests = bounds.batch_requests.min(b.batch_requests);
            bounds.input_bytes = bounds.input_bytes.min(b.input_bytes);
            bounds.output_bytes = bounds.output_bytes.min(b.output_bytes);
        }
        let requested = limits.unwrap_or_else(|| json!({}));
        let req = requested
            .as_object()
            .ok_or_else(|| Error("resource limits must be a mapping".into()))?;
        let mut limits = json!({"steps":1_000_000,"depth":128,"digits":256});
        for (key, max) in [("steps", 10_000_000), ("depth", 4096), ("digits", 4096)] {
            if let Some(v) = req.get(key) {
                require(
                    v.as_u64().is_some_and(|v| v > 0 && v <= max),
                    &format!("invalid resource limit: {key}"),
                )?;
                limits[key] = v.clone();
            }
        }
        Ok(Self {
            snapshot,
            runtime,
            requested,
            limits,
            bounds,
            basis: None,
        })
    }
    fn input_basis(&mut self) -> Result<&mut InputBasis> {
        if self.basis.is_none() {
            self.basis = Some(InputBasis::new(self.snapshot.data())?);
        }
        Ok(self.basis.as_mut().unwrap())
    }
    pub fn basis(&mut self, id: &str) -> Result<V> {
        self.input_basis()?.basis(id)
    }
    fn operations(&self) -> J {
        json!({"timeout_seconds":self.bounds.timeout.as_secs(),"batch_requests":self.bounds.batch_requests,"input_bytes":self.bounds.input_bytes,"output_bytes":self.bounds.output_bytes})
    }
    fn empty_result(&self, query: bool, resources: J) -> J {
        let mut result = json!({"schema_version":if query{2}else{1},"profile":"core/v1","modules":if query{vec!["query/v1"]}else{vec![]},"snapshot_id":self.snapshot.snapshot_id(),"computation_id":null,"implementation":null,"resource_profile":resources,"operational_limits":self.operations(),"status":"unknown","value":null,"diagnostics":[],"executed_reads":[],"potential_dependencies":[],"potential_ids":[],"basis":null,"assurance":{"kind":"computed","formal_scope":[]},"cost":zero_cost(query)});
        if query {
            result["query_counts"] = J::Null;
        }
        result
    }
    fn scalar_resources(&self) -> J {
        let mut r = self.limits.clone();
        r["version"] = json!("resources/v2");
        r
    }
    fn capabilities(&self) -> Result<J> {
        reasoning_fields::capabilities(&map(self.snapshot.data())?["document"], Some("core/v1"))
            .and_then(|v| v.to_json())
            .map_err(cap_error)
    }
    fn interpretation(result: &mut J, cap: &J) {
        if cap["experimental_override"] == true {
            result["interpretation"] =
                json!({"declared_profile":cap["declared_profile"],"explicit_override":"core/v1"});
        }
    }
    fn prepare_query(
        &mut self,
        authored: &V,
        declared: &[String],
        root_id: Option<&str>,
    ) -> Result<(Option<J>, J)> {
        let resources = match Q::resources(Some(&self.requested)) {
            Ok(r) => r,
            Err(_) => {
                let mut r = self.empty_result(true, Q::resources(None)?);
                r["status"] = json!("error");
                r["diagnostics"] = json!([qdiagnostic("invalid_limits", [])]);
                return Ok((None, r));
            }
        };
        let mut result = self.empty_result(true, resources.clone());
        let cap = self.capabilities()?;
        Self::interpretation(&mut result, &cap);
        let scope = map(authored)
            .ok()
            .and_then(|m| m.get("query"))
            .and_then(|v| map(v).ok())
            .and_then(|m| m.get("scope"))
            .and_then(|v| {
                if let V::Text(s) = v {
                    Some(s.clone())
                } else {
                    None
                }
            });
        let mut required = BTreeSet::new();
        let mut root = J::Null;
        if let Some(id) = root_id {
            required.insert(id.to_owned());
            let summary = self.input_basis()?.summary(id)?.to_json()?;
            root = json!({"kind":"node","id":id,"fingerprint":summary["fingerprint"]});
            result["potential_dependencies"] = json!([root]);
            result["potential_ids"] = json!([id]);
            if summary["status"] != "available" {
                result["status"] = json!("error");
                result["diagnostics"] = json!(
                    strings(&summary["diagnostics"])?
                        .iter()
                        .map(|c| qdiagnostic(c, [id.to_owned()]))
                        .collect::<Vec<_>>()
                );
                return Ok((None, result));
            }
        } else if let Some(scope) = &scope {
            required.insert(scope.clone());
        }
        let undeclared = required
            .into_iter()
            .filter(|id| !declared.contains(id))
            .collect::<Vec<_>>();
        if !undeclared.is_empty() {
            result["status"] = json!("error");
            result["diagnostics"] = json!([qdiagnostic("undeclared_dependency", undeclared)]);
            return Ok((None, result));
        }
        let Some(scope) = scope.filter(|s| !s.is_empty()) else {
            result["status"] = json!("error");
            result["diagnostics"] = json!([qdiagnostic("invalid_expression", [])]);
            return Ok((None, result));
        };
        let work = (|| -> Result<J> {
            let capture = self.snapshot.capture_query_scope(&scope, None)?;
            let witness = capture.witness().to_json()?;
            let mut dependencies = Vec::new();
            if !root.is_null() {
                dependencies.push(root.clone());
            }
            dependencies.push(witness.clone());
            result["potential_dependencies"] = json!(dependencies);
            let mut ids = root_id.into_iter().map(str::to_owned).collect::<Vec<_>>();
            ids.push(scope.clone());
            ids.sort();
            result["potential_ids"] = json!(ids);
            let fields = strings(&capture.definition().to_json()?["fields"])?;
            let normalized = Q::lower(&authored.to_json()?, &fields)?;
            let modules = Q::required_modules(&normalized);
            result["modules"] = json!(modules);
            let declared_modules = strings(&cap["requires"])?;
            require(
                modules.iter().all(|m| declared_modules.contains(m)),
                "capability:unsupported_capability",
            )?;
            let id = hash(
                &json!({"version":1,"snapshot_id":self.snapshot.snapshot_id(),"root_witness":root,"scope_witness":witness,"operation":normalized,"resources":resources}),
            )?;
            let prepared = QA::prepare(
                &capture,
                authored,
                &id,
                &declared_modules,
                Some(&root),
                Some(&resources),
            )?;
            QA::validate_prepared(&prepared)?;
            result["modules"] = prepared["required_modules"].clone();
            result["resource_profile"] = prepared["request"]["resources"].clone();
            result["potential_dependencies"] = prepared["potential_dependencies"].clone();
            result["potential_ids"] = prepared["potential_ids"].clone();
            let cost = &prepared["basis_template"]["preflight_cost"];
            for key in ["candidates", "field_reads", "preflight_steps"] {
                result["cost"][key] = cost[key].clone();
            }
            if let Some(id) = root_id {
                result["cost"]["node_evaluations"][id] = json!(1);
            }
            Ok(prepared)
        })();
        match work {
            Ok(p) => Ok((Some(p), result)),
            Err(e) => {
                if e.0 == "transport_limit" {
                    return Err(Error("batch_input_limit".into()));
                }
                let capability = e.0.starts_with("capability:");
                let raw = e.0.strip_prefix("capability:").unwrap_or(&e.0);
                let code = if raw.starts_with("limit") {
                    if raw.contains("field") {
                        "field_read_limit"
                    } else {
                        "candidate_limit"
                    }
                } else if [
                    "candidate_limit",
                    "field_read_limit",
                    "depth_limit",
                    "value_limit",
                    "digit_limit",
                    "step_limit",
                    "incomplete_history_scope",
                    "invalid_scope",
                    "scope_unavailable",
                ]
                .contains(&raw)
                    || capability
                {
                    raw
                } else if raw.starts_with("undeclared modules:") {
                    "unsupported_capability"
                } else {
                    "invalid_expression"
                };
                result["status"] = json!(if capability || code == "unsupported_capability" {
                    "unsupported_capability"
                } else if code.ends_with("_limit") {
                    "limit"
                } else {
                    "error"
                });
                result["diagnostics"] = json!([qdiagnostic(code, [scope])]);
                Ok((None, result))
            }
        }
    }
    pub fn prepare(&mut self, expression: &V, declared: &[String]) -> Result<(Option<J>, J)> {
        if is_query(expression) {
            return self.prepare_query(expression, declared, None);
        }
        if let V::Map(m) = expression
            && m.len() == 1
            && let Some(V::Text(id)) = m.get("ref")
        {
            let rule = map(&map(self.snapshot.data())?["nodes"])?
                .get(id)
                .and_then(|v| map(v).ok())
                .and_then(|m| m.get("body"))
                .and_then(|v| map(v).ok())
                .and_then(|m| m.get("rule"))
                .filter(|v| is_query(v))
                .cloned();
            if let Some(rule) = rule {
                return self.prepare_query(&rule, declared, Some(id));
            }
        }
        require(
            self.requested
                .as_object()
                .unwrap()
                .keys()
                .all(|k| ["steps", "depth", "digits"].contains(&k.as_str())),
            "unknown resource limits",
        )?;
        let cap = self.capabilities()?;
        let tree = L::lower(expression).map_err(|e| {
            if e.0 == "unsupported_capability" {
                cap_error(e)
            } else {
                e
            }
        })?;
        let direct = L::references(&tree);
        let undeclared = direct
            .iter()
            .filter(|id| !declared.contains(id))
            .cloned()
            .collect::<Vec<_>>();
        let component = L::closure(self.snapshot.data(), &direct)?;
        let mut modules = L::required_modules(&tree)
            .into_iter()
            .collect::<BTreeSet<_>>();
        for node in component["nodes"].as_object().unwrap().values() {
            modules.extend(L::required_modules(node));
        }
        modules.extend(self.input_basis()?.modules(&direct)?);
        let declared_modules = strings(&cap["requires"])?;
        require(
            modules.iter().all(|m| declared_modules.contains(m)),
            "capability:unsupported_capability",
        )?;
        let composed = modules.contains("composition/v1");
        let modules = modules.into_iter().collect::<Vec<_>>();
        let mut limits = self.limits.clone();
        if composed {
            limits["value_nodes"] = json!(10_000);
            limits["value_depth"] = json!(128);
            limits["value_bytes"] = json!(16_777_216);
        }
        let mut resources = limits.clone();
        resources["version"] = json!(if composed {
            "resources/v3"
        } else {
            "resources/v2"
        });
        let witnesses = self.input_basis()?.dependencies(&direct)?.to_json()?;
        let potential = witnesses
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        let as_of = map(self.snapshot.data())?["as_of"].to_json()?;
        let mut basis = json!({"version":1,"recipe":"merkle-inputs/v1","profile":"core/v1","modules":modules,"as_of":as_of,"expression":tree,"dependencies":witnesses});
        basis["digest"] = json!(hash(&basis)?);
        let id = hash(
            &json!({"snapshot_id":self.snapshot.snapshot_id(),"expression":tree,"profile":"core/v1","modules":modules,"declared":declared.iter().cloned().collect::<BTreeSet<_>>(),"as_of":as_of,"resources":resources}),
        )?;
        let mut result = self.empty_result(false, resources);
        result["modules"] = json!(modules);
        result["computation_id"] = json!(id);
        result["potential_dependencies"] = witnesses;
        result["potential_ids"] = json!(potential);
        result["basis"] = basis;
        Self::interpretation(&mut result, &cap);
        if !undeclared.is_empty() {
            result["status"] = json!("error");
            result["diagnostics"] = json!([diagnostic("undeclared_dependency", undeclared)]);
            return Ok((None, result));
        }
        if !component["errors"].as_object().unwrap().is_empty() {
            result["status"] = json!("error");
            result["diagnostics"] = json!(
                component["errors"]
                    .as_object()
                    .unwrap()
                    .iter()
                    .map(|(id, c)| diagnostic(c.as_str().unwrap(), [id.clone()]))
                    .collect::<Vec<_>>()
            );
            return Ok((None, result));
        }
        let mut view = SnapshotView::new(self.snapshot, &potential, &[], None);
        for id in component["nodes"].as_object().unwrap().keys() {
            require(
                view.read_node(id)? != V::Null,
                "module input was not captured",
            )?;
        }
        let mut request = json!({"nodes":component["nodes"],"expression":tree,"declared":potential,"limits":limits});
        if composed {
            request["protocol"] = json!("KP3");
        }
        Ok((Some(request), result))
    }
    pub fn evaluate_many(&mut self, requests: &[(V, Vec<String>)]) -> Result<Vec<J>> {
        let mut prepared = Vec::new();
        let (mut input_size, mut output_size) = (0usize, 0usize);
        for (expression, declared) in requests {
            require(
                prepared.len() < self.bounds.batch_requests,
                "batch_request_limit",
            )?;
            let item = match self.prepare(expression, declared) {
                Ok(item) => item,
                Err(e) => {
                    if e.0 == "batch_input_limit" {
                        return Err(e);
                    }
                    let code =
                        e.0.strip_prefix("capability:")
                            .unwrap_or("invalid_expression");
                    let mut result = self.empty_result(false, self.scalar_resources());
                    result["status"] = json!(if code == "unsupported_capability" {
                        "unsupported_capability"
                    } else {
                        "error"
                    });
                    result["diagnostics"] = json!([diagnostic(code, [])]);
                    (None, result)
                }
            };
            input_size += bytes(item.0.as_ref().unwrap_or(&J::Null))?;
            output_size += bytes(&item.1)?;
            require(input_size <= self.bounds.input_bytes, "batch_input_limit")?;
            require(output_size <= self.bounds.output_bytes, "output_limit")?;
            prepared.push(item);
        }
        let active = prepared
            .iter()
            .enumerate()
            .filter_map(|(i, (r, _))| r.as_ref().map(|r| (i, r.clone())))
            .collect::<Vec<_>>();
        if !active.is_empty() {
            let executed = (|| -> Result<Vec<(usize, J, J, J)>> {
                let runtime = self
                    .runtime
                    .ok_or_else(|| Error("runtime_unavailable".into()))?;
                let wire = active
                    .iter()
                    .map(|(_, r)| {
                        if r["protocol"] == "KP4" {
                            r["request"].clone()
                        } else {
                            r.clone()
                        }
                    })
                    .collect::<Vec<_>>();
                let responses = runtime.request_many_bounded(&wire, &self.bounds)?;
                require(
                    responses.len() == active.len(),
                    "runtime response count mismatch",
                )?;
                let mut accepted = Vec::new();
                for ((i, request), response) in active.iter().zip(responses) {
                    let (response, basis) = if request["protocol"] == "KP4" {
                        QA::response_and_basis(&response, request)?
                    } else {
                        let declared = strings(&request["declared"])?;
                        let potential = strings(&prepared[*i].1["potential_ids"])?;
                        require(
                            strings(&response["executed_reads"])?
                                .iter()
                                .all(|s| declared.contains(s))
                                && strings(&response["potential_reads"])?
                                    .iter()
                                    .all(|s| potential.contains(s)),
                            "native dependency witness disagrees with captured closure",
                        )?;
                        (response, J::Null)
                    };
                    accepted.push((*i, response, basis, runtime.implementation_for(request)?));
                }
                Ok(accepted)
            })();
            match executed {
                Err(e) => {
                    if ["batch_request_limit", "batch_input_limit", "output_limit"].contains(&e.0.as_str()) {
                        return Err(e);
                    }
                    let code = if e.0 == "runtime_timeout" {
                        "runtime_timeout"
                    } else {
                        "runtime_unavailable"
                    };
                    for (i, request) in &active {
                        let result = &mut prepared[*i].1;
                        let query = request["protocol"] == "KP4";
                        result["status"] = json!("operational_error");
                        result["value"] = J::Null;
                        result["diagnostics"] = json!([if query {
                            qdiagnostic(code, [])
                        } else {
                            diagnostic(code, [])
                        }]);
                        result["executed_reads"] = json!([]);
                        result["basis"] = J::Null;
                        result["computation_id"] = J::Null;
                        result["cost"] = zero_cost(query);
                        result["assurance"] = json!({"kind":"computed","formal_scope":[]});
                    }
                }
                Ok(accepted) => {
                    for (i, response, basis, implementation) in accepted {
                        let request = prepared[i].0.as_ref().unwrap().clone();
                        let mut reads = Vec::new();
                        if request["protocol"] != "KP4" {
                            for id in strings(&response["executed_reads"])? {
                                let summary = self.input_basis()?.summary(&id)?.to_json()?;
                                reads.push(json!({"kind":"node","id":id,"fingerprint":summary["fingerprint"]}));
                            }
                        }
                        let result = &mut prepared[i].1;
                        result["status"] = response["status"].clone();
                        result["value"] = response["value"].clone();
                        result["implementation"] = implementation.clone();
                        result["assurance"]["implementation"] = json!(hash(&implementation)?);
                        if request["protocol"] == "KP4" {
                            result["diagnostics"] = response["diagnostics"].clone();
                            result["query_counts"] = response["query_counts"].clone();
                            result["executed_reads"] = response["executed_reads"].clone();
                            result["cost"] = response["cost"].clone();
                            result["computation_id"] = if basis.is_null() {
                                J::Null
                            } else {
                                json!(hash(
                                    &json!({"snapshot_id":self.snapshot.snapshot_id(),"basis":basis,"resources":result["resource_profile"]})
                                )?)
                            };
                            result["basis"] = basis;
                        } else {
                            result["diagnostics"] = json!(
                                strings(&response["diagnostics"])?
                                    .iter()
                                    .map(|c| diagnostic(c, []))
                                    .collect::<Vec<_>>()
                            );
                            result["executed_reads"] = json!(reads);
                            result["cost"] = json!({"steps":response["steps"],"preflight_steps":response["preflight_steps"],"node_evaluations":response["node_evaluations"]});
                            let mut closed = true;
                            let mut pending = vec![&request["expression"]];
                            while let Some(e) = pending.pop() {
                                if e.get("num").is_none()
                                    && !e["op"]
                                        .as_str()
                                        .is_some_and(|s| ["add", "sub", "mul", "div"].contains(&s))
                                {
                                    closed = false;
                                    break;
                                }
                                if let Some(args) = e.get("args").and_then(J::as_array) {
                                    pending.extend(args);
                                }
                            }
                            if closed && result["status"] == "ok" {
                                result["assurance"]["formal_scope"] =
                                    json!(["Kpopper.Proof.evaluate_closedRat_sound"]);
                            }
                        }
                    }
                }
            }
        }
        let results = prepared.into_iter().map(|(_, r)| r).collect::<Vec<_>>();
        require(
            bytes(&json!(results))? <= self.bounds.output_bytes,
            "output_limit",
        )?;
        Ok(results)
    }
    pub fn evaluate(&mut self, expression: V, declared: Vec<String>) -> Result<J> {
        Ok(self.evaluate_many(&[(expression, declared)])?.remove(0))
    }
}
