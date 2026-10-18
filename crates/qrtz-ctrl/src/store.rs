//! ctrl's on-disk state (slice 2, ADR 0103): two kinds of append-only, hash-chained JSON-lines files under
//! `<app data>/ctrl/`:
//!
//! - `tasks.jsonl`: the task store's log (`task.created`, `task.paused`, `task.deleted`, `run.recorded`). The
//!   task list is a pure fold over it ([`fold_store`]), like a run's state is a fold over its events.
//! - `runs/<run id>.jsonl`: one [`EventLog`] per run, a record per line, written as it happens.
//!
//! A line is written whole and `fsync`ed before the call returns. A crash mid-write leaves a torn last line (no
//! `\n`); opening drops it and truncates the file back to the last whole record. Anything else that doesn't
//! verify (a bad line in the middle, a broken hash chain, a wrong seq) is damage: the file is never rewritten,
//! the store refuses writes and says so. Ids come from the store's own seq (`task-7`, `run-12`), so a replay of
//! the same log gives the same ids.
use crate::log::{Event, EventLog, Record};
use crate::task::{clean_text, Schedule, Task, TaskError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

pub const STORE_SCHEMA_V: u32 = 1;
const GENESIS: &str = "0000000000000000000000000000000000000000000000000000000000000000";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StoreEvent {
    #[serde(rename = "task.created")]
    TaskCreated {
        task_id: String,
        text: String,
        schedule: Schedule,
    },
    #[serde(rename = "task.paused")]
    TaskPaused { task_id: String, paused: bool },
    #[serde(rename = "task.deleted")]
    TaskDeleted { task_id: String },
    /// A run of this task exists in `runs/<run_id>.jsonl`.
    #[serde(rename = "run.recorded")]
    RunRecorded { task_id: String, run_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoreRecord {
    pub seq: u64,
    pub t_wall_ms: u64,
    pub schema_v: u32,
    pub event: StoreEvent,
    pub prev_hash: String,
    pub hash: String,
}

fn store_digest(r: &StoreRecord) -> String {
    #[derive(Serialize)]
    struct Body<'a> {
        seq: u64,
        t_wall_ms: u64,
        schema_v: u32,
        event: &'a StoreEvent,
        prev_hash: &'a str,
    }
    let body = Body {
        seq: r.seq,
        t_wall_ms: r.t_wall_ms,
        schema_v: r.schema_v,
        event: &r.event,
        prev_hash: &r.prev_hash,
    };
    hex::encode(Sha256::digest(
        serde_json::to_vec(&body).expect("records serialize"),
    ))
}

/// The task list, derived. `order` is creation order; deleted tasks leave it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoreState {
    pub tasks: BTreeMap<String, Task>,
    pub order: Vec<String>,
    /// run id → (task id, when it was recorded).
    pub runs: BTreeMap<String, (String, u64)>,
}

/// The store reducer. Pure: the time is the record's.
pub fn apply_store(mut s: StoreState, r: &StoreRecord) -> StoreState {
    match &r.event {
        StoreEvent::TaskCreated {
            task_id,
            text,
            schedule,
        } => {
            s.order.push(task_id.clone());
            s.tasks.insert(
                task_id.clone(),
                Task {
                    id: task_id.clone(),
                    text: text.clone(),
                    schedule: *schedule,
                    paused: false,
                    created_at: r.t_wall_ms,
                    runs: Vec::new(),
                },
            );
        }
        StoreEvent::TaskPaused { task_id, paused } => {
            if let Some(t) = s.tasks.get_mut(task_id) {
                t.paused = *paused;
            }
        }
        StoreEvent::TaskDeleted { task_id } => {
            s.tasks.remove(task_id);
            s.order.retain(|t| t != task_id);
        }
        StoreEvent::RunRecorded { task_id, run_id } => {
            if let Some(t) = s.tasks.get_mut(task_id) {
                t.runs.push(run_id.clone());
            }
            s.runs.insert(run_id.clone(), (task_id.clone(), r.t_wall_ms));
        }
    }
    s
}

pub fn fold_store<'a>(records: impl IntoIterator<Item = &'a StoreRecord>) -> StoreState {
    records.into_iter().fold(StoreState::default(), apply_store)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    NotFound,
    Invalid(TaskError),
    /// A file that doesn't verify. `file` is the name inside the ctrl folder, `line` is 0-based.
    Damaged {
        file: String,
        line: u64,
    },
    Io(io::ErrorKind),
}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e.kind())
    }
}

/// Read the whole lines of a JSON-lines file; a torn last line (no `\n`) is cut off the file. Missing = empty.
fn read_whole_lines(path: &Path) -> io::Result<Vec<String>> {
    let mut f = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let whole = buf.iter().rposition(|b| *b == b'\n').map_or(0, |i| i + 1);
    if whole < buf.len() {
        f.set_len(whole as u64)?;
        f.sync_all()?;
    }
    Ok(String::from_utf8_lossy(&buf[..whole])
        .lines()
        .map(str::to_string)
        .collect())
}

fn append_line(path: &Path, value: &impl Serialize) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    let mut o = OpenOptions::new();
    o.create(true).append(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    let mut f: File = o.open(path)?;
    f.write_all(&line)?;
    f.sync_data()
}

fn private_dir(dir: &Path) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// A run id is only ever one the store minted (`run-<seq>`): no path can come in through it.
fn is_run_id(id: &str) -> bool {
    id.strip_prefix("run-")
        .is_some_and(|n| !n.is_empty() && n.len() <= 20 && n.bytes().all(|b| b.is_ascii_digit()))
}

pub struct Store {
    dir: PathBuf,
    records: Vec<StoreRecord>,
    state: StoreState,
}

impl Store {
    /// Open (or create) the store in `dir`, verifying the task log's chain.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Store, StoreError> {
        let dir = dir.into();
        private_dir(&dir)?;
        private_dir(&dir.join("runs"))?;
        let mut records: Vec<StoreRecord> = Vec::new();
        let mut prev = GENESIS.to_string();
        for (i, line) in read_whole_lines(&dir.join("tasks.jsonl"))?.iter().enumerate() {
            let damaged = || StoreError::Damaged {
                file: "tasks.jsonl".into(),
                line: i as u64,
            };
            let r: StoreRecord = serde_json::from_str(line).map_err(|_| damaged())?;
            if r.seq != i as u64 || r.prev_hash != prev || store_digest(&r) != r.hash {
                return Err(damaged());
            }
            prev = r.hash.clone();
            records.push(r);
        }
        let state = fold_store(&records);
        Ok(Store { dir, records, state })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn records(&self) -> &[StoreRecord] {
        &self.records
    }

    pub fn state(&self) -> &StoreState {
        &self.state
    }

    /// Tasks, newest first.
    pub fn tasks(&self) -> Vec<&Task> {
        self.state
            .order
            .iter()
            .rev()
            .filter_map(|id| self.state.tasks.get(id))
            .collect()
    }

    pub fn task(&self, id: &str) -> Result<&Task, StoreError> {
        self.state.tasks.get(id).ok_or(StoreError::NotFound)
    }

    fn next_seq(&self) -> u64 {
        self.records.len() as u64
    }

    fn append(&mut self, t_wall_ms: u64, event: StoreEvent) -> Result<(), StoreError> {
        let mut r = StoreRecord {
            seq: self.next_seq(),
            t_wall_ms,
            schema_v: STORE_SCHEMA_V,
            event,
            prev_hash: self
                .records
                .last()
                .map_or_else(|| GENESIS.to_string(), |r| r.hash.clone()),
            hash: String::new(),
        };
        r.hash = store_digest(&r);
        append_line(&self.dir.join("tasks.jsonl"), &r)?;
        self.state = apply_store(std::mem::take(&mut self.state), &r);
        self.records.push(r);
        Ok(())
    }

    pub fn create_task(&mut self, now_ms: u64, text: &str, schedule: Schedule) -> Result<Task, StoreError> {
        let text = clean_text(text).map_err(StoreError::Invalid)?;
        schedule.validate().map_err(StoreError::Invalid)?;
        let task_id = format!("task-{}", self.next_seq());
        self.append(
            now_ms,
            StoreEvent::TaskCreated {
                task_id: task_id.clone(),
                text,
                schedule,
            },
        )?;
        self.task(&task_id).cloned()
    }

    pub fn set_paused(&mut self, now_ms: u64, id: &str, paused: bool) -> Result<Task, StoreError> {
        if self.task(id)?.paused != paused {
            self.append(
                now_ms,
                StoreEvent::TaskPaused {
                    task_id: id.into(),
                    paused,
                },
            )?;
        }
        self.task(id).cloned()
    }

    pub fn delete_task(&mut self, now_ms: u64, id: &str) -> Result<(), StoreError> {
        self.task(id)?;
        self.append(now_ms, StoreEvent::TaskDeleted { task_id: id.into() })
    }

    /// Start a run of `task_id`: `run.recorded` in the store, then the run's own log with its first events.
    pub fn start_run(
        &mut self,
        now_ms: u64,
        task_id: &str,
        first: &[(u64, Event)],
    ) -> Result<EventLog, StoreError> {
        self.task(task_id)?;
        let run_id = format!("run-{}", self.next_seq());
        self.append(
            now_ms,
            StoreEvent::RunRecorded {
                task_id: task_id.into(),
                run_id: run_id.clone(),
            },
        )?;
        let mut log = EventLog::new(&run_id);
        for (mono, e) in first {
            self.append_run_record(&mut log, *mono, now_ms, e.clone())?;
        }
        Ok(log)
    }

    /// Append one event to a run: in memory and on disk (the file is the truth; memory follows it).
    pub fn append_run_record(
        &self,
        log: &mut EventLog,
        t_mono_ns: u64,
        t_wall_ms: u64,
        event: Event,
    ) -> Result<(), StoreError> {
        let r: Record = log.append(t_mono_ns, t_wall_ms, event).clone();
        append_line(&self.run_path(log.run_id()), &r)?;
        Ok(())
    }

    /// Persist records already appended to `log` in memory (from index `from`): the executor's runner appends the
    /// records of one action, and they are written right after it (slice 5).
    pub fn persist_tail(&self, log: &EventLog, from: usize) -> Result<(), StoreError> {
        for r in log.records().iter().skip(from) {
            append_line(&self.run_path(log.run_id()), r)?;
        }
        Ok(())
    }

    fn run_path(&self, run_id: &str) -> PathBuf {
        self.dir.join("runs").join(format!("{run_id}.jsonl"))
    }

    /// Load a run from disk and verify its chain.
    pub fn run(&self, run_id: &str) -> Result<EventLog, StoreError> {
        if !is_run_id(run_id) || !self.state.runs.contains_key(run_id) {
            return Err(StoreError::NotFound);
        }
        let file = format!("runs/{run_id}.jsonl");
        let lines = read_whole_lines(&self.run_path(run_id))?;
        let log = EventLog::from_jsonl(run_id, &lines.join("\n")).map_err(|_| StoreError::Damaged {
            file: file.clone(),
            line: 0,
        })?;
        log.verify().map_err(|e| StoreError::Damaged {
            file,
            line: match e {
                crate::log::LogError::BrokenChain { seq } => seq,
                crate::log::LogError::BadSeq { expected, .. } => expected,
            },
        })?;
        Ok(log)
    }

    /// The task a run belongs to and when it was recorded.
    pub fn run_meta(&self, run_id: &str) -> Option<&(String, u64)> {
        self.state.runs.get(run_id)
    }
}
