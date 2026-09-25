//! Positive source-clock ancestry proved by exact Git commit objects. Publication
//! parents, timestamps, refs, replace objects and shallow boundaries are not proof.
use crate::{Result, history_contract::*, require, value::TypedValue as V};
use sha2::Digest;
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::Path,
};

pub const PREFIX: &str = "evidence/source-clocks/";
const MAX_COMMITS: usize = 4096;
const MAX_CAPTURE: usize = 256;
const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_WORK: usize = 4_000_000;

pub(crate) fn oid(id: &str) -> bool {
    [40, 64].contains(&id.len())
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(crate) fn path_id(path: &str) -> Option<&str> {
    path.strip_prefix(PREFIX)
        .and_then(|s| s.strip_suffix(".commit"))
        .filter(|s| oid(s))
}
fn identity(raw: &[u8], width: usize) -> String {
    let header = format!("commit {}\0", raw.len());
    if width == 40 {
        let mut h = sha1::Sha1::new();
        h.update(header);
        h.update(raw);
        format!("{:x}", h.finalize())
    } else {
        let mut h = sha2::Sha256::new();
        h.update(header);
        h.update(raw);
        format!("{:x}", h.finalize())
    }
}
fn parents(id: &str, raw: &[u8]) -> Result<Vec<String>> {
    require(oid(id) && raw.len() <= MAX_BYTES, "source_ancestry_limit")?;
    require(identity(raw, id.len()) == id, "source_ancestry_hash")?;
    let end = raw
        .windows(2)
        .position(|b| b == b"\n\n")
        .ok_or_else(|| error("source_ancestry_commit"))?;
    let mut lines = raw[..end].split(|b| *b == b'\n');
    let tree = lines
        .next()
        .and_then(|b| b.strip_prefix(b"tree "))
        .and_then(|b| std::str::from_utf8(b).ok())
        .ok_or_else(|| error("source_ancestry_commit"))?;
    require(
        oid(tree) && tree.len() == id.len(),
        "source_ancestry_commit",
    )?;
    let mut found = Vec::new();
    let mut headers = false;
    for line in lines {
        if let Some(parent) = line.strip_prefix(b"parent ") {
            // Git parent headers precede author/committer and extended headers.
            require(!headers, "source_ancestry_commit")?;
            let parent =
                std::str::from_utf8(parent).map_err(|_| error("source_ancestry_commit"))?;
            require(
                oid(parent)
                    && parent.len() == id.len()
                    && parent != id
                    && !found.iter().any(|p| p == parent),
                "source_ancestry_commit",
            )?;
            found.push(parent.to_owned());
        } else {
            headers = true;
            require(
                !line.is_empty() && !line.contains(&0),
                "source_ancestry_commit",
            )?;
        }
    }
    Ok(found)
}

#[derive(Clone, Debug, Default)]
pub struct Graph {
    edges: BTreeMap<String, Vec<String>>,
}
impl Graph {
    pub(crate) fn insert(&mut self, path: &str, raw: &[u8]) -> Result<()> {
        let id = path_id(path).ok_or_else(|| error("source_ancestry_path"))?;
        let p = parents(id, raw)?;
        require(
            self.edges.get(id).is_none_or(|old| old == &p),
            "source_ancestry_collision",
        )?;
        self.edges.insert(id.into(), p);
        require(self.edges.len() <= MAX_COMMITS, "source_ancestry_limit")
    }
    pub(crate) fn merge(&mut self, other: &Self) -> Result<()> {
        for (id, p) in &other.edges {
            require(
                self.edges.get(id).is_none_or(|old| old == p),
                "source_ancestry_collision",
            )?;
            self.edges.insert(id.clone(), p.clone());
        }
        require(self.edges.len() <= MAX_COMMITS, "source_ancestry_limit")
    }
    /// The verified parents of one admitted commit, if admitted.
    pub(crate) fn parents_of(&self, id: &str) -> Option<&Vec<String>> {
        self.edges.get(id)
    }
    pub(crate) fn select<'a>(&self, paths: impl Iterator<Item = &'a String>) -> Self {
        Self {
            edges: paths
                .filter_map(|p| path_id(p))
                .filter_map(|id| self.edges.get(id).map(|p| (id.to_owned(), p.clone())))
                .collect(),
        }
    }
    /// Charge traversals across the entire reducer call and refuse exhaustion, never
    /// silently convert a resource failure into "unrelated".
    pub(crate) fn with_ancestry<T>(
        &self,
        apply: impl FnOnce(&crate::source_clock::Ancestry<'_>) -> Result<T>,
    ) -> Result<T> {
        let work = Cell::new(0usize);
        let exhausted = Cell::new(false);
        let ancestry = |a: &V, b: &V| {
            let (V::Text(a), V::Text(b)) = (a, b) else {
                return false;
            };
            if !oid(a) || !oid(b) || a.len() != b.len() {
                return false;
            }
            let mut todo = vec![b.as_str()];
            let mut seen = BTreeSet::new();
            while let Some(id) = todo.pop() {
                work.set(work.get().saturating_add(1));
                if work.get() > MAX_WORK {
                    exhausted.set(true);
                    return false;
                }
                if id == a {
                    return true;
                }
                if seen.insert(id) {
                    if let Some(parents) = self.edges.get(id) {
                        todo.extend(parents.iter().map(String::as_str));
                    }
                }
            }
            false
        };
        let result = apply(&ancestry);
        require(!exhausted.get(), "source_ancestry_work_limit")?;
        result
    }
}

/// A bounded positive path, whose original commit bytes can be retained once by
/// content identity. Full OIDs are required; ambiguous or mutable names are refused.
pub struct Proof {
    files: BTreeMap<String, Vec<u8>>,
}
impl Proof {
    pub fn capture(repo: &Path, ancestor: &str, descendant: &str) -> Result<Self> {
        require(
            oid(ancestor)
                && oid(descendant)
                && ancestor.len() == descendant.len()
                && ancestor != descendant,
            "source_ancestry_endpoints",
        )?;
        let mut todo = VecDeque::from([descendant.to_owned()]);
        let mut seen = BTreeSet::new();
        let mut child = BTreeMap::<String, String>::new();
        let mut raw = BTreeMap::new();
        let mut total = 0usize;
        while let Some(id) = todo.pop_front() {
            if !seen.insert(id.clone()) {
                continue;
            }
            require(seen.len() <= MAX_CAPTURE, "source_ancestry_limit")?;
            let bytes =
                crate::history_branch_git::git(repo, &["cat-file", "commit", &id], MAX_BYTES)?;
            total = total.saturating_add(bytes.len());
            require(total <= MAX_BYTES, "source_ancestry_limit")?;
            let ps = parents(&id, &bytes)?;
            raw.insert(id.clone(), bytes);
            if id == ancestor {
                let mut files = BTreeMap::new();
                let mut current = ancestor.to_owned();
                loop {
                    files.insert(
                        format!("{PREFIX}{current}.commit"),
                        raw.remove(&current).unwrap(),
                    );
                    if current == descendant {
                        break;
                    }
                    current = child[&current].clone();
                }
                return Ok(Self { files });
            }
            for parent in ps {
                child.entry(parent.clone()).or_insert_with(|| id.clone());
                todo.push_back(parent);
            }
        }
        Err(error("source_ancestry_unproven"))
    }
    pub(crate) fn files(&self) -> &BTreeMap<String, Vec<u8>> {
        &self.files
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    fn commit(parent: Option<&str>, message: &str) -> (String, Vec<u8>) {
        let raw = format!("tree {}\n{}author Fixture <fixture@example.test> 1 +0000\ncommitter Fixture <fixture@example.test> 1 +0000\n\n{message}\n", "0".repeat(40), parent.map(|p| format!("parent {p}\n")).unwrap_or_default()).into_bytes();
        (identity(&raw, 40), raw)
    }
    pub(crate) fn graph() -> (Graph, String, String, String) {
        let (a, _) = commit(None, "base");
        let (b, raw) = commit(Some(&a), "next");
        let path = format!("{PREFIX}{b}.commit");
        let mut graph = Graph::default();
        graph.insert(&path, &raw).unwrap();
        (graph, a, b, path)
    }
    #[test]
    fn commit_bytes_not_observed_edges_authenticate_parenthood() {
        let (graph, a, b, _) = graph();
        graph
            .with_ancestry(|ancestry| {
                assert!(ancestry(&V::Text(a.clone()), &V::Text(b.clone())));
                assert!(!ancestry(&V::Text(b.clone()), &V::Text(a.clone())));
                assert!(!ancestry(&V::Text(a[..12].into()), &V::Text(b.clone())));
                Ok(())
            })
            .unwrap();
        let (_, wrong) = commit(Some(&a), "altered");
        assert_eq!(
            Graph::default()
                .insert(&format!("{PREFIX}{b}.commit"), &wrong)
                .unwrap_err()
                .0,
            "source_ancestry_hash"
        );
    }
    #[test]
    fn source_clock_work_exhaustion_refuses_instead_of_hiding_ancestry() {
        let (graph, a, b, _) = graph();
        let a = V::Text(a);
        let b = V::Text(b);
        assert_eq!(
            graph
                .with_ancestry(|ancestry| {
                    for _ in 0..MAX_WORK {
                        ancestry(&a, &b);
                    }
                    Ok(())
                })
                .unwrap_err()
                .0,
            "source_ancestry_work_limit"
        );
    }
    #[test]
    fn oversized_source_clock_group_refuses_before_any_ancestry_callback() {
        let mut objects = Map::new();
        for i in 0..2001 {
            let mut object = V::from_json(&serde_json::json!({"schema_version":2,"id_scheme":"typed-history/v2",
                "subject":"p.clock","kind":"reading","body":{"v":i,"from":"repo"},"at":{"commit":format!("{i:040x}")},
                "by":"fixture","on":"2026-09-24","op":format!("create-{i}"),"saw":[],"pins":{},
                "authored":{"collection":"known","profile":"ordinary-reader/v1","fields":{"deps":"rests_on","snapshot":"seen","predicate":"wrong_if"}}})).unwrap();
            let id = crate::identity::typed_object_identity(&object).unwrap();
            crate::history_view::map_mut(&mut object)
                .unwrap()
                .insert("id".into(), V::Text(id.clone()));
            objects.insert(id, object);
        }
        let calls = Cell::new(0);
        let ancestry = |_: &V, _: &V| {
            calls.set(calls.get() + 1);
            false
        };
        let error = crate::history_reduce::reduce(&objects, None, Some(&ancestry)).unwrap_err();
        assert_eq!(error.0, "history_limit");
        assert_eq!(calls.get(), 0);
    }
}
