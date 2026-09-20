//! Capture the real ordinary Hub page projection used by arrangement writes.
//!
//! The record and its `.view.yaml` are observed once. Write adapters must retain
//! this capture through publication and persist its view/routing evidence in
//! their recovery baseline. An absent view produces an empty facts map.

use crate::{
    Error, Result,
    history_transaction::Layout,
    ordinary_assessment, ordinary_assessment_report, ordinary_hub,
    reasoning_runtime::Runtime,
    source_capture::{self, ReadMode},
    source_inventory::{Inventory, absolute, name},
    value::TypedValue as V,
};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
};

#[derive(Clone)]
pub(crate) struct PageCapture {
    pub facts: V,
    pub view_path: PathBuf,
    pub view_before: Option<Vec<u8>>,
    pub view_inventory: Inventory,
    pub source: Rc<source_capture::CapturedSource>,
    pub routing: V,
}

fn view_path(entry: &Path) -> Result<PathBuf> {
    let entry = absolute(entry)?;
    let file = entry
        .file_name()
        .ok_or_else(|| Error("invalid_path".into()))?;
    let layout = Layout::for_entry(name(Path::new(file))?)?;
    Ok(entry
        .parent()
        .ok_or_else(|| Error("invalid_path".into()))?
        .join(layout.view))
}

impl PageCapture {
    pub(crate) fn capture(
        paths: &[PathBuf],
        cwd: &Path,
        as_of: Option<V>,
        runtime: Option<&Runtime>,
    ) -> Result<Self> {
        let entry = paths
            .first()
            .ok_or_else(|| Error("missing_record_path".into()))?;
        let view_path = view_path(entry)?;
        let captured = source_capture::capture_source_with_runtime(
            paths,
            cwd,
            ReadMode::Live,
            as_of,
            runtime,
        )?;
        let mut view_inventory = Inventory::default();
        let view_before = if view_inventory.exists(&view_path)? {
            Some(view_inventory.read(&view_path)?)
        } else {
            None
        };
        let facts = if let Some(view_before) = &view_before {
            let report = ordinary_assessment_report::from_capture(
                &captured,
                runtime,
                ordinary_assessment::POLICY,
            )?;
            ordinary_hub::build(
                &captured,
                &report,
                Some(view_before),
                entry,
                &view_path,
                16 * 1024 * 1024,
                runtime,
            )?
            .arrangement_facts
        } else {
            V::Map(crate::history_contract::Map::new())
        };
        let routing = source_capture::routing_observation(paths, cwd)?;
        captured.verify()?;
        view_inventory.verify()?;
        Ok(Self {
            facts,
            view_path,
            view_before,
            view_inventory,
            source: Rc::new(captured),
            routing,
        })
    }

    pub(crate) fn verify(&self) -> Result<()> {
        self.view_inventory.verify()?;
        self.source.verify()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history_contract::map;
    use std::fs;

    fn s(value: &str) -> V {
        V::Text(value.into())
    }

    #[test]
    fn actual_python_page_facts_match_all_page_counter_families() {
        let cases: serde_json::Value =
            serde_json::from_slice(include_bytes!("../tests/fixtures/ordinary-page-facts.json"))
                .unwrap();
        for case in cases.as_array().unwrap() {
            let temp = tempfile::tempdir().unwrap();
            for (relative, raw) in case["files"].as_object().unwrap() {
                let path = temp.path().join(relative);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(path, raw.as_str().unwrap()).unwrap();
            }
            let entry = temp.path().join("GROUNDING.yaml");
            let captured =
                PageCapture::capture(&[entry], temp.path(), Some(s("2026-09-19")), None).unwrap();
            assert_eq!(
                captured.facts.to_json().unwrap(),
                case["facts"],
                "{}",
                case["name"]
            );
            captured.verify().unwrap();
        }
    }

    #[test]
    fn captures_real_view_facts_and_detects_a_changed_view() {
        let temp = tempfile::tempdir().unwrap();
        let entry = temp.path().join("GROUNDING.yaml");
        fs::write(
            &entry,
            "meta:\n  updated: 2026-09-01\nsources:\n  s.q: {asked: Inspect this record}\nknown:\n  p.a: {v: 1}\njudgments:\n  d.arr:\n    verdict: continue\n    rests_on: [s.q, graph.entries]\n    seen: {s.q: Inspect this record, graph.entries: 1}\n    wrong_if: graph.entries > 0\n",
        ).unwrap();
        fs::create_dir_all(temp.path().join(".kpopper")).unwrap();
        let view = temp.path().join(".kpopper/view.yaml");
        fs::write(
            &view,
            "tabs:\n- title: Decision\n  serves: [s.q]\n  sections:\n  - {title: d.arr, pick: judgments, as: cards}\n",
        ).unwrap();
        let captured = PageCapture::capture(
            std::slice::from_ref(&entry),
            temp.path(),
            Some(s("2026-09-20")),
            None,
        )
        .unwrap();
        let facts = map(&captured.facts).unwrap();
        let arrangement = map(facts.get("d.arr").unwrap()).unwrap();
        assert_eq!(arrangement.get("linked"), Some(&V::Bool(true)));
        assert_eq!(arrangement.get("fired"), Some(&V::Bool(true)));
        assert_eq!(arrangement.get("reading"), Some(&s("graph.entries is 2")));
        captured.verify().unwrap();
        fs::write(&view, "tabs: []\n").unwrap();
        assert!(captured.verify().is_err());
        fs::write(
            &view,
            "tabs:\n- title: Wrong tab\n  serves: [s.other]\n  sections:\n  - {title: d.arr, pick: judgments, as: cards}\n",
        )
        .unwrap();
        let cut = PageCapture::capture(
            std::slice::from_ref(&entry),
            temp.path(),
            Some(s("2026-09-20")),
            None,
        )
        .unwrap();
        let cut_facts = map(&cut.facts).unwrap();
        assert_eq!(
            map(cut_facts.get("d.arr").unwrap()).unwrap().get("linked"),
            Some(&V::Bool(false))
        );

        let absent_root = tempfile::tempdir().unwrap();
        let absent_entry = absent_root.path().join("GROUNDING.yaml");
        fs::write(&absent_entry, "known:\n  p.a: {v: 1}\n").unwrap();
        let absent = PageCapture::capture(
            std::slice::from_ref(&absent_entry),
            absent_root.path(),
            Some(s("2026-09-20")),
            None,
        )
        .unwrap();
        fs::create_dir_all(absent_root.path().join(".kpopper")).unwrap();
        fs::write(absent_root.path().join(".kpopper/view.yaml"), "tabs: []\n").unwrap();
        assert!(absent.verify().is_err());
    }
}
