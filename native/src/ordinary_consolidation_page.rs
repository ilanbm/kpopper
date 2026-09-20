//! Full-domain read-only page evidence, using the renderer's selection/counting
//! algorithm. Presentation-only fields are retained by its shared input parser.
use crate::{
    Result,
    ordinary_value::{Map, Value as V, list, map, py, text, truth},
    ordinary_views::{HubData, Projection},
    reasoning_runtime::Runtime,
    source_capture::OrdinaryCapture,
};
use std::collections::{BTreeMap, BTreeSet};
// The shared parser retains fields used by HTML rendering, while this adapter
// consumes only the selection and evidence result.
#[allow(dead_code)]
mod shared {
    use super::*;
    include!("ordinary_hub_page_inputs.rs");
    include!("ordinary_hub_page_projection.rs");
    fn decode_brief(raw: &[u8]) -> Result<V> {
        Ok(crate::history_yaml::decode_full_ordinary_source_value(raw)?.projected())
    }
    pub(crate) fn facts(
        capture: &OrdinaryCapture,
        raw: &[u8],
        runtime: Option<&Runtime>,
    ) -> Result<V> {
        let document = capture.ordinary_document();
        let context = V::from_typed(&capture.ordinary_context());
        let assessment = crate::ordinary_report::assess(
            document,
            map(capture.hypotheses())?,
            &context,
            runtime,
            crate::ordinary_assessment::POLICY,
        )?;
        let report = map(&assessment)?;
        let nodes = map(&report["nodes"])?;
        let fields = crate::ordinary_fields::snapshot_fields(document)?;
        let dep = fields
            .get("deps")
            .and_then(|v| text(v).ok())
            .unwrap_or("rests_on");
        let ids = nodes.keys().cloned().collect::<BTreeSet<_>>();
        let judgments = ids
            .iter()
            .filter(|id| body(nodes, id).is_ok_and(|b| b.contains_key(dep)))
            .cloned()
            .collect::<BTreeSet<_>>();
        let (brief, tabs) = parse_brief(Some(raw))?;
        let mut projection = Projection::new(
            document,
            map(capture.hypotheses())?,
            map(&map(&context)?["conflicts"])?,
            capture.reader_lines()?,
            runtime,
        )?;
        let hub = projection.hub_data()?;
        let states = ids
            .iter()
            .map(|id| (id.clone(), hub.flags.get(id).cloned().unwrap_or_default()))
            .collect();
        Ok(V::Map(
            page_projection(
                &brief,
                &tabs,
                &ids,
                &judgments,
                &states,
                nodes,
                dep,
                &mut projection,
            )?
            .arrangement_facts,
        ))
    }
}
pub(super) use shared::facts;
