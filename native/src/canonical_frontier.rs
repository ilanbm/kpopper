//! Derived navigation summaries; source membership and evidence stay unchanged.
use crate::{Error, Result, identity::sha256};
use serde_json::{Value as J, json};
use std::collections::{BTreeMap, BTreeSet};

pub(super) fn depth(path: &str) -> usize {
    path.split('/').filter(|part| !part.is_empty()).count()
}

fn digest(value: &J) -> Result<String> {
    Ok(sha256(&serde_json::to_vec(value)?))
}

pub(super) struct Tree<'a> {
    pub revision: &'a str,
    pub scope: &'a str,
    pub groups: &'a BTreeMap<String, BTreeSet<String>>,
    pub children: &'a BTreeMap<String, BTreeSet<String>>,
    pub direct: &'a BTreeMap<String, Vec<String>>,
}

impl Tree<'_> {
    fn details(&self, path: &str) -> Result<J> {
        crate::require(
            self.groups.contains_key(path),
            "unknown navigation child path",
        )?;
        let mut children = Vec::new();
        for child in self.children.get(path).into_iter().flatten() {
            let members = self
                .groups
                .get(child)
                .ok_or_else(|| Error("navigation child lacks membership".into()))?;
            children.push(json!([
                format!("group:{child}"),
                members.len(),
                digest(&json!(members))?
            ]));
        }
        let direct = self
            .direct
            .get(path)
            .into_iter()
            .flatten()
            .cloned()
            .collect::<BTreeSet<_>>();
        Ok(json!({"group":format!("group:{path}"),"children":children,"direct_ids":direct}))
    }

    fn binding(&self, details: &J) -> Result<String> {
        digest(
            &json!({"schema":"kpopper.navigation-children/v1","revision":self.revision,
            "scope":self.scope,"details":details}),
        )
    }

    pub fn summary(&self, path: &str, members: &J) -> Result<J> {
        let details = self.details(path)?;
        let hash = self.binding(&details)?;
        Ok(json!({
            "group":format!("group:{path}"),
            "member_count":members.as_array().map_or(0,Vec::len),
            "members_sha256":digest(members)?,
            "children_count":details["children"].as_array().unwrap().len(),
            "direct_count":details["direct_ids"].as_array().unwrap().len(),
            "children_sha256":hash,
            "expand":format!("children:{hash}:group:{path}")
        }))
    }

    pub fn expand(&self, handle: &str) -> Result<J> {
        let (binding, group) = handle
            .strip_prefix("children:")
            .and_then(|value| value.split_once(":group:"))
            .ok_or_else(|| {
                Error(
                    "invalid navigation child handle; use the issued revision-bound handle".into(),
                )
            })?;
        let details = self.details(group)?;
        crate::require(
            binding == self.binding(&details)?,
            "navigation child handle belongs to a different revision, scope or membership",
        )?;
        let mut result = details;
        let mut child_summaries = Vec::new();
        for child in self.children.get(group).into_iter().flatten() {
            child_summaries.push(self.summary(child, &json!(self.groups[child]))?);
        }
        result["children"] = json!(child_summaries);
        result["handle"] = json!(handle);
        result["additional_bodies_read"] = json!(false);
        Ok(result)
    }

    pub fn hidden_descendants(&self, folded: &BTreeSet<String>) -> BTreeSet<String> {
        let mut hidden = BTreeSet::new();
        let mut queue = folded
            .iter()
            .flat_map(|path| self.children.get(path).into_iter().flatten().cloned())
            .collect::<Vec<_>>();
        while let Some(path) = queue.pop() {
            if hidden.insert(path.clone()) {
                queue.extend(self.children.get(&path).into_iter().flatten().cloned());
            }
        }
        hidden
    }
}
