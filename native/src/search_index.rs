//! A disposable private index. Record reads and source permission checks run before
//! publication. A failed or superseded cache transaction never changes a record.
use crate::{identity::sha256, public_search_rank as rank};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde_json::Value;
use std::{cell::RefCell, rc::Rc};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const READER_BUILD: &str = concat!(
    env!("CARGO_PKG_VERSION"),
    "/",
    env!("KPOP_REASONING_ADAPTER_SHA256"),
    ":"
);

pub(crate) struct Index {
    db: Connection,
    generation: i64,
    repair: bool,
    pub previous_world: String,
    pub previous: BTreeMap<String, (String, Value)>,
    pub world: String,
    pub inputs: BTreeMap<String, String>,
    calculations: Rc<RefCell<Calculations>>,
}

#[derive(Default)]
struct Calculations {
    saved: BTreeMap<String, Vec<u8>>,
    used: BTreeMap<String, Vec<u8>>,
    failed: bool,
}
thread_local! {static ACTIVE: RefCell<Option<Rc<RefCell<Calculations>>>>=const {RefCell::new(None)};}
pub(crate) struct ComputationScope(Option<Rc<RefCell<Calculations>>>);
impl Drop for ComputationScope {
    fn drop(&mut self) {
        ACTIVE.with_borrow_mut(|old| *old = self.0.take());
    }
}
pub(crate) fn lookup_frame(
    implementation: &Value,
    frame: &[u8],
) -> Option<(String, Option<Vec<u8>>)> {
    ACTIVE.with_borrow(|slot| {
        let memo = slot.as_ref()?;
        let key = sha256(&serde_json::to_vec(&(implementation, sha256(frame))).ok()?);
        let mut memo = memo.borrow_mut();
        if memo.failed {
            return None;
        }
        let hit = memo
            .used
            .get(&key)
            .or_else(|| memo.saved.get(&key))
            .cloned();
        if let Some(data) = &hit {
            memo.used.insert(key.clone(), data.clone());
        }
        Some((key, hit))
    })
}
pub(crate) fn save_frame(key: String, data: &[u8]) {
    ACTIVE.with_borrow(|slot| {
        let Some(memo) = slot else { return };
        let mut memo = memo.borrow_mut();
        if !memo.failed {
            memo.used.insert(key, data.to_vec());
        }
    });
}

/// A transport frame also needs the evaluator's dependency witness checks.
/// An operational error is a valid finding but cannot seed later evaluations.
pub(crate) fn discard_frames() {
    ACTIVE.with_borrow(|slot| {
        if let Some(memo) = slot {
            let mut memo = memo.borrow_mut();
            memo.failed = true;
            memo.used.clear();
        }
    });
}

fn location(record: &Path) -> Option<PathBuf> {
    let root = crate::public_workspace::runtime_cache()
        .ok()?
        .join("search-v1");
    let key = sha256(record.to_str()?.as_bytes());
    root.is_absolute()
        .then(|| root.join(format!("{key}.sqlite")))
}

fn row_digest(reference: &str, input: &str, data: &str) -> String {
    sha256(
        &[
            reference.as_bytes(),
            b"\0",
            input.as_bytes(),
            b"\0",
            data.as_bytes(),
        ]
        .concat(),
    )
}

impl Index {
    pub fn computing(&self) -> ComputationScope {
        ComputationScope(ACTIVE.with_borrow_mut(|slot| slot.replace(self.calculations.clone())))
    }
    pub fn open(record: &Path, create: bool) -> Option<Self> {
        let path = location(record)?;
        if !create && !path.is_file() {
            return None;
        }
        Self::open_path(&path).ok()
    }

    fn open_path(path: &Path) -> Result<Self> {
        let parent = path.parent().ok_or("no index directory")?;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent)?;
        let meta = fs::symlink_metadata(parent)?;
        if !meta.is_dir() {
            return Err("invalid index directory".into());
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if meta.permissions().mode() & 0o077 != 0 {
                return Err("index directory is not private".into());
            }
        }
        if let Ok(meta) = fs::symlink_metadata(path)
            && !meta.is_file()
        {
            return Err("invalid index file".into());
        }
        let path = parent
            .canonicalize()?
            .join(path.file_name().ok_or("invalid index path")?);
        let mut db = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_CREATE
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        db.busy_timeout(Duration::from_millis(750))?;
        // WAL permits concurrent readers of the last complete generation. A
        // filesystem that has not enabled WAL can still use ordinary snapshots.
        let journal: String = db.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
        if journal != "wal" {
            let _: String = db.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
        }
        db.execute_batch("PRAGMA trusted_schema=OFF;
            CREATE TABLE IF NOT EXISTS head(singleton INTEGER PRIMARY KEY CHECK(singleton=1), generation INTEGER NOT NULL, revision TEXT NOT NULL, world TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS documents(id INTEGER PRIMARY KEY, ref TEXT UNIQUE NOT NULL, pos INTEGER NOT NULL, text_hash TEXT NOT NULL, input TEXT NOT NULL, data_hash TEXT NOT NULL, data TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS calculations(key TEXT PRIMARY KEY,digest TEXT NOT NULL,frame BLOB NOT NULL);
            CREATE VIRTUAL TABLE IF NOT EXISTS evidence USING fts5(identity, name, content, tokenize='unicode61');")?;
        let tx = db.transaction()?;
        let (generation, stored_world): (i64, String) = tx
            .query_row(
                "SELECT generation,world FROM head WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?
            .unwrap_or_default();
        let mut previous = BTreeMap::new();
        let compatible = stored_world.strip_prefix(READER_BUILD);
        let previous_world = compatible.unwrap_or_default().to_owned();
        let mut repair: bool = tx.query_row(
            "SELECT (SELECT count(*) FROM documents) != (SELECT count(*) FROM evidence)",
            [],
            |r| r.get(0),
        )?;
        repair |= generation != 0 && compatible.is_none();
        {
            let mut query =
                tx.prepare("SELECT ref,input,data_hash,data FROM documents WHERE input != ''")?;
            let mut rows = query.query([])?;
            while let Some(row) = rows.next()? {
                let reference: String = row.get(0)?;
                let input: String = row.get(1)?;
                let digest: String = row.get(2)?;
                let data: String = row.get(3)?;
                if let Some(id) = reference.strip_prefix("node:")
                    && row_digest(&reference, &input, &data) == digest
                    && let Ok(value) = serde_json::from_str::<Value>(&data)
                    && value["ref"] == reference
                    && value["id"] == id
                {
                    previous.insert(id.to_owned(), (input, value));
                } else {
                    repair = true;
                }
            }
        }
        let mut calculations = Calculations::default();
        {
            let mut stmt = tx.prepare("SELECT key,digest,frame FROM calculations")?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                let key: String = row.get(0)?;
                let digest: String = row.get(1)?;
                let frame: Vec<u8> = row.get(2)?;
                if sha256(&[key.as_bytes(), &frame].concat()) == digest {
                    calculations.saved.insert(key, frame);
                } else {
                    repair = true;
                }
            }
        }
        if compatible.is_none() {
            previous.clear();
            calculations.saved.clear();
        }
        tx.commit()?;
        Ok(Self {
            db,
            generation,
            repair,
            previous_world,
            previous,
            world: String::new(),
            inputs: BTreeMap::new(),
            calculations: Rc::new(RefCell::new(calculations)),
        })
    }

    /// Rank against the very generation being published. A slower earlier writer
    /// cannot replace a newer generation prepared by a different process.
    pub fn synchronize(
        &mut self,
        corpus: &Value,
        query: Option<&str>,
    ) -> Result<Option<Vec<usize>>> {
        let rows = corpus["rows"].as_array().ok_or("missing rows")?;
        let revision = corpus["revision"].as_str().ok_or("missing revision")?;
        let world = format!("{READER_BUILD}{}", self.world);
        if !self.repair && !self.calculations.borrow().failed {
            let read = self.db.transaction()?;
            let current: bool = read.query_row(
                "SELECT EXISTS(SELECT 1 FROM head WHERE singleton=1 AND revision=?1 AND world=?2)",
                params![revision, world],
                |r| r.get(0),
            )?;
            if current {
                let hits = ranked(&read, query)?;
                read.commit()?;
                return Ok(Some(hits));
            }
            // End the read snapshot before competing to publish the new one.
        }
        let tx = self
            .db
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let head: Option<(i64, String, String)> = tx
            .query_row(
                "SELECT generation,revision,world FROM head WHERE singleton=1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        let generation = head.as_ref().map_or(0, |h| h.0);
        if self.repair
            || self.calculations.borrow().failed
            || head
                .as_ref()
                .is_none_or(|h| h.1 != revision || h.2 != world)
        {
            if generation != self.generation {
                return Ok(None);
            }
            if self.repair {
                tx.execute("DELETE FROM evidence", [])?;
                tx.execute("DELETE FROM documents", [])?;
                tx.execute("DELETE FROM calculations", [])?;
            }
            let mut existing = BTreeMap::new();
            {
                let mut statement =
                    tx.prepare("SELECT ref,id,pos,text_hash,input,data_hash FROM documents")?;
                let mut records = statement.query([])?;
                while let Some(r) = records.next()? {
                    existing.insert(
                        r.get::<_, String>(0)?,
                        (
                            r.get::<_, i64>(1)?,
                            r.get::<_, usize>(2)?,
                            r.get::<_, String>(3)?,
                            r.get::<_, String>(4)?,
                            r.get::<_, String>(5)?,
                        ),
                    );
                }
            }
            for (i, row) in rows.iter().enumerate() {
                let pos = i + 1;
                let reference = row["ref"].as_str().ok_or("missing row reference")?;
                let id = row["id"].as_str().ok_or("missing row ID")?;
                let name = row["name"].as_str();
                let content = rank::search_text(row)?;
                let text_hash = sha256(&serde_json::to_vec(&(id, name, &content))?);
                let input = reference
                    .strip_prefix("node:")
                    .and_then(|id| self.inputs.get(id))
                    .map(String::as_str)
                    .unwrap_or("");
                // Only semantic node rows are reusable. Source and capture text is
                // always revalidated by the originating reader and grant.
                let data = if input.is_empty() {
                    String::new()
                } else {
                    serde_json::to_string(row)?
                };
                let data_hash = row_digest(reference, input, &data);
                let rowid = if let Some((rowid, oldpos, oldtext, oldinput, olddigest)) =
                    existing.remove(reference)
                {
                    if oldinput != input || olddigest != data_hash || oldtext != text_hash {
                        tx.execute("UPDATE documents SET pos=?1,text_hash=?2,input=?3,data_hash=?4,data=?5 WHERE id=?6",
                            params![pos,text_hash,input,data_hash,data,rowid])?;
                    } else if oldpos != pos {
                        tx.execute(
                            "UPDATE documents SET pos=?1 WHERE id=?2",
                            params![pos, rowid],
                        )?;
                    }
                    if oldtext == text_hash {
                        continue;
                    }
                    tx.execute("DELETE FROM evidence WHERE rowid=?1", [rowid])?;
                    rowid
                } else {
                    tx.execute("INSERT INTO documents(ref,pos,text_hash,input,data_hash,data) VALUES(?1,?2,?3,?4,?5,?6)",
                        params![reference,pos,text_hash,input,data_hash,data])?;
                    tx.last_insert_rowid()
                };
                tx.execute(
                    "INSERT INTO evidence(rowid,identity,name,content) VALUES(?1,?2,?3,?4)",
                    params![rowid, id, name, content],
                )?;
            }
            for (_, (rowid, ..)) in existing {
                tx.execute("DELETE FROM evidence WHERE rowid=?1", [rowid])?;
                tx.execute("DELETE FROM documents WHERE id=?1", [rowid])?;
            }
            let calculations = self.calculations.borrow();
            for (key, frame) in &calculations.used {
                if self.repair || !calculations.saved.contains_key(key) {
                    tx.execute(
                        "INSERT OR REPLACE INTO calculations(key,digest,frame) VALUES(?1,?2,?3)",
                        params![key, sha256(&[key.as_bytes(), frame].concat()), frame],
                    )?;
                }
            }
            for key in calculations
                .saved
                .keys()
                .filter(|key| !calculations.used.contains_key(*key))
            {
                tx.execute("DELETE FROM calculations WHERE key=?1", [key])?;
            }
            tx.execute("INSERT OR REPLACE INTO head(singleton,generation,revision,world) VALUES(1,?1,?2,?3)",
                params![generation+1,revision,world])?;
        }
        let hits = ranked(&tx, query)?;
        tx.commit()?;
        Ok(Some(hits))
    }
}

fn ranked(db: &Connection, query: Option<&str>) -> Result<Vec<usize>> {
    let terms = query.map(rank::terms).unwrap_or_default();
    if terms.is_empty() {
        return Ok(Vec::new());
    }
    let mut stmt = db.prepare(
        "SELECT d.pos FROM evidence JOIN documents d ON d.id=evidence.rowid
        WHERE evidence MATCH ? ORDER BY bm25(evidence,8,4,1),d.pos",
    )?;
    Ok(stmt
        .query_map([rank::expression(&terms)], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}
