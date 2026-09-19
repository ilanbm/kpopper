//! One validated immutable Snapshot, computational report and consumer view.
//! Replayed contexts rebuild every projection before callers may consume them.
use crate::{
    Error, Result,
    history_contract::*,
    history_view::map_mut,
    reasoning_history_assessment as H, reasoning_projection as P,
    reasoning_runtime::{OperationalBounds, Runtime},
    reasoning_snapshot::{Snapshot, digest},
    require,
    value::TypedValue as V,
};
use serde_json::{Value as J, json};
const MAX_VALUES: usize = 64 * 1024 * 1024 / 8;
fn s(v: &str) -> V {
    V::Text(v.into())
}
fn val(j: J) -> Result<V> {
    V::from_json_bounded(&j, MAX_VALUES)
}
pub fn capture_failure(detail: &str, code: Option<&str>, stage: &str) -> Result<V> {
    let derived = detail
        .split(':')
        .next()
        .unwrap_or("")
        .trim()
        .to_lowercase()
        .replace(' ', "_");
    let code = code
        .filter(|s| !s.is_empty())
        .unwrap_or(if derived.is_empty() {
            "capture_failed"
        } else {
            &derived
        });
    let mut result = val(
        json!({"schema_version":1,"kind":"assessment_capture_failure","assessment_profile":"core/v1","stage":stage,"code":code,"detail":detail,"findings":null}),
    )?;
    let hash = digest(&result)?;
    map_mut(&mut result)?.insert("failure_revision".into(), s(&hash));
    Ok(result)
}
pub fn validate_failure(value: &V) -> Result<V> {
    let m = schema(
        value,
        &[
            "schema_version",
            "kind",
            "assessment_profile",
            "stage",
            "code",
            "detail",
            "findings",
            "failure_revision",
        ],
        &[],
    )?;
    require(
        crate::source_clock::python_equal(&m["schema_version"], &val(json!(1))?)
            && string_is(&m["kind"], "assessment_capture_failure")
            && string_is(&m["assessment_profile"], "core/v1")
            && m["findings"] == V::Null,
        "invalid assessment capture failure",
    )?;
    for k in ["stage", "code", "detail"] {
        require(
            !text(&m[k])?.is_empty(),
            "invalid assessment capture failure",
        )?;
    }
    let mut preimage = m.clone();
    preimage.remove("failure_revision");
    require(
        m["failure_revision"] == s(&digest(&V::Map(preimage))?),
        "noncanonical assessment capture failure",
    )?;
    Ok(value.clone())
}
#[derive(Clone, Debug)]
pub struct CapturedAssessment {
    snapshot: Snapshot,
    base: V,
    assessment: V,
    view: V,
}
impl CapturedAssessment {
    pub fn new(snapshot: Snapshot, assessment: &V) -> Result<Self> {
        let assessment = H::validate(assessment)?;
        let r = map(&assessment)?;
        require(
            r["snapshot_id"] == s(snapshot.snapshot_id()),
            "assessment belongs to a different snapshot",
        )?;
        let mut base = Map::new();
        for k in [
            "assessment_profile",
            "attention_policy",
            "snapshot_id",
            "as_of",
            "scope",
            "operational_limits",
        ] {
            base.insert(k.into(), r[k].clone());
        }
        base.insert("schema_version".into(), val(json!(2))?);
        base.insert("selection".into(), r["assessment_selection"].clone());
        base.insert(
            "assessment_revision".into(),
            r["base_assessment_revision"].clone(),
        );
        let nodes = map(&r["nodes"])?
            .iter()
            .map(|(id, n)| {
                let n = map(n)?;
                Ok((
                    id.clone(),
                    V::Map(
                        ["body", "fields", "state", "attention", "computation"]
                            .iter()
                            .map(|k| ((*k).into(), n[*k].clone()))
                            .collect(),
                    ),
                ))
            })
            .collect::<Result<Map>>()?;
        base.insert("nodes".into(), V::Map(nodes));
        let base = H::validate_v2(&snapshot, &V::Map(base))?;
        let display = crate::history_view::list(&r["display_selection"])?
            .iter()
            .map(|v| text(v).map(str::to_owned))
            .collect::<Result<Vec<_>>>()?;
        require(
            H::from_v2(&snapshot, &base, Some(&display))? == assessment,
            "assessment does not match retained snapshot",
        )?;
        let view = P::project_findings(&assessment)?;
        let v = map(&view)?;
        require(
            v["snapshot_id"] == s(snapshot.snapshot_id())
                && v["findings_revision"] == r["findings_revision"],
            "consumer view belongs to different assessment",
        )?;
        Ok(Self {
            snapshot,
            base,
            assessment,
            view,
        })
    }
    pub fn from_snapshot(
        snapshot: Snapshot,
        selection: Option<&[String]>,
        policy: &str,
        runtime: Option<&Runtime>,
        bounds: OperationalBounds,
        display: Option<&[String]>,
    ) -> Result<Self> {
        let report = H::assess(&snapshot, selection, policy, runtime, bounds, display)?;
        Self::new(snapshot, &report)
    }
    pub fn snapshot(&self) -> &Snapshot {
        &self.snapshot
    }
    pub fn snapshot_id(&self) -> &str {
        self.snapshot.snapshot_id()
    }
    pub fn findings_revision(&self) -> &str {
        text(&map(&self.assessment).unwrap()["findings_revision"]).unwrap()
    }
    pub fn base_assessment(&self) -> &V {
        &self.base
    }
    pub fn assessment(&self) -> &V {
        &self.assessment
    }
    pub fn view(&self) -> &V {
        &self.view
    }
    pub fn session_revision(&self, project_identity: &V) -> Result<String> {
        digest(&V::Map(Map::from([
            ("version".into(), val(json!(1))?),
            ("project_identity".into(), project_identity.clone()),
            ("snapshot_id".into(), s(self.snapshot_id())),
            ("findings_revision".into(), s(self.findings_revision())),
            ("consumer_view_version".into(), s(P::VERSION)),
        ])))
    }
    pub fn to_data(&self) -> Result<J> {
        let payload = V::Map(Map::from([
            ("snapshot".into(), s(&self.snapshot.to_json()?)),
            ("assessment".into(), self.assessment.clone()),
            ("view".into(), self.view.clone()),
        ]));
        Ok(
            json!({"version":1,"encoding":"typed-json/v1","payload":payload.to_tagged_bounded(MAX_VALUES)?,"context_revision":digest(&payload)?}),
        )
    }
    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(&self.to_data()?)?)
    }
    pub fn from_data(payload: &J) -> Result<Self> {
        let m = payload
            .as_object()
            .ok_or_else(|| Error("invalid captured assessment context".into()))?;
        require(
            m.len() == 4
                && ["version", "encoding", "payload", "context_revision"]
                    .iter()
                    .all(|k| m.contains_key(*k))
                && (m["version"] == 1 || m["version"] == true)
                && m["encoding"] == "typed-json/v1",
            "invalid captured assessment context",
        )?;
        let decoded = V::from_tagged_bounded(&m["payload"], MAX_VALUES)?;
        require(
            decoded.to_tagged_bounded(MAX_VALUES)? == m["payload"],
            "noncanonical captured payload",
        )?;
        let d = schema(&decoded, &["snapshot", "assessment", "view"], &[])?;
        require(
            m["context_revision"] == digest(&decoded)?,
            "noncanonical captured context",
        )?;
        let context = Self::new(
            Snapshot::from_json(text(&d["snapshot"])?.as_bytes())?,
            &d["assessment"],
        )?;
        require(
            context.view == d["view"],
            "captured consumer view does not match findings",
        )?;
        Ok(context)
    }
    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        require(bytes.len() <= 512 * 1024 * 1024, "context_limit")?;
        let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
        for byte in bytes {
            if quoted {
                if escaped {
                    escaped = false
                } else if *byte == b'\\' {
                    escaped = true
                } else if *byte == b'"' {
                    quoted = false
                }
            } else {
                match byte {
                    b'"' => quoted = true,
                    b'[' | b'{' => {
                        depth += 1;
                        require(depth <= 400, "context_limit")?
                    }
                    b']' | b'}' => depth = depth.saturating_sub(1),
                    _ => {}
                }
            }
        }
        let mut de = serde_json::Deserializer::from_slice(bytes);
        de.disable_recursion_limit();
        let value: J = serde::Deserialize::deserialize(&mut de)?;
        de.end()?;
        Self::from_data(&value)
    }
}
