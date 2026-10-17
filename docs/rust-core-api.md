# Rust core API: `qrtz-storage` and `qrtz-lifecycle` (stage 1 sketch)

_Status: API sketch for review. It holds no crate code yet. Owner: Bozniak (storage + lifecycle crates).
The Tauri shell, `qrtz_invoke`/`qrtz_subscribe`, the helper child process, e2e and perf belong to Bobs.
Companion to `docs/recording-lifecycle.md` (behaviour contract) and the migration plan (P2/P3)._

Everything here is a port of code on `main` at `c64174f`. The TS sources this mirrors are:

| Concern                                                | TS source of truth                                                                          |
| ------------------------------------------------------ | ------------------------------------------------------------------------------------------- |
| Migrations runner                                      | `packages/core/src/db/migrate.ts` (`migrate`, `getSchemaVersion`, `migrations`)             |
| Schema (v2–v5)                                         | `packages/core/src/note/schema.ts` (`noteMigrations`)                                       |
| Connection pragmas, health                             | `apps/desktop/src/worker/db.ts`                                                             |
| Repo + FTS queries                                     | `packages/core/src/note/repo.ts` (`NoteRepo`, `toFtsQuery`)                                 |
| Lifecycle/pipeline persistence, steps, list/get/import | `apps/desktop/src/worker/handlers.ts` (`lifecycle.*`, `pipeline.*`, `note.*`, `data.*`)     |
| Reducers, pipeline, retry                              | `packages/core/src/note/lifecycle.ts`                                                       |
| Runtime owner                                          | `apps/desktop/src/main/note-service.ts` (`NoteService`)                                     |
| IPC contract                                           | `packages/core/src/ipc/contract.ts`, `events.ts`, rendered to `contracts/ipc-contract.json` |

## 1. Layout: one cargo workspace at the repo root

```
Cargo.toml                 # [workspace] members = ["crates/*"]  (+ "apps/desktop-tauri/src-tauri" when Bobs adds it)
Cargo.lock                 # committed; one lock for crates + shell
rust-toolchain.toml        # channel = "1.90" (see "Toolchain")
crates/
  qrtz-model/              # wire types only: serde (+ optional specta). No IO, no tokio, no rusqlite.
  qrtz-storage/            # rusqlite (bundled, FTS5): migrations, NoteRepo, LifecycleRepo, snapshot/import
  qrtz-lifecycle/          # pure reducers + pipeline + retry (default), runtime driver behind feature "runtime" (tokio)
spec/vectors/lifecycle/    # shared JSON test vectors, run by vitest AND cargo test
spec/fixtures/db/          # fixture DBs written by the TS repo (parity tests both ways)
```

**Why the root, not `src-tauri/crates`:**

- `src-tauri` does not exist yet (checked `git log`/`ls` at `c64174f`). The plan puts it under
  `apps/desktop-tauri/src-tauri`, so it simply joins the root workspace as a member. Tauri 2 supports being a
  workspace member; it uses the workspace `target/`.
- The crates must stay UI-free. A future `qrtz-eval` CLI, a headless integration harness (Bobs's Rust
  integration tests), and any other binary can depend on them without pulling in Tauri.
- One `Cargo.lock` and one `target/` for everything: `cargo test --workspace` from the root is the single gate,
  next to `npm test`.
- Kill switch from the plan stays one `rm -rf crates/ apps/desktop-tauri/` plus deleting the root `Cargo.toml`.

**Why a third tiny crate (`qrtz-model`):** storage needs the domain types (`Meeting`, `Segment`, `MeetingSummary`,
`PipelineState`, ...) and lifecycle needs some of them too. Putting them in either crate forces the other to depend
on it: lifecycle would compile bundled SQLite just to get `MeetingPlatform`, or storage would depend on the
state machine. `qrtz-model` has only `serde`/`serde_json` (and `specta` behind a feature). It is also the single
place TS types get generated from after cutover.

Dependency graph (no cycles):

```
qrtz-model  <-  qrtz-storage
qrtz-model  <-  qrtz-lifecycle  (--features runtime: + tokio, async-trait)
qrtz-storage + qrtz-lifecycle  <-  src-tauri (Bobs) / qrtz-core glue: implements the lifecycle traits with storage
```

`qrtz-lifecycle` deliberately does **not** depend on `qrtz-storage`. It defines the traits it needs, and the
glue implements them (an `impl LifecycleStore for StorageHandle` lives in the shell or a future `qrtz-core`;
I can ship it in `qrtz-storage` behind a `lifecycle` feature if Bobs prefers).

**Toolchain.** The box has rustc 1.85.1. `specta 2.0.0-rc.25` (the current tauri-specta line) fails to build on
1.85 (it uses `extract_if`, stabilised later), and Tauri 2's own MSRV keeps rising. Pin a recent stable in
`rust-toolchain.toml` (proposed `1.90`). Bobs: please confirm the version your shell builds with, and I'll match.

**Versions (from `cargo search` today):** `rusqlite 0.40` (`bundled`, FTS5 is in the bundled build), `tokio 1.53`,
`serde 1`, `serde_json 1`, `thiserror 2`, `async-trait 0.1`, `unicode-normalization 0.1`, `indexmap 2`,
`uuid 1`, optional `specta =2.0.0-rc.25`. **SQLite parity:** Electron 44's `node:sqlite` is **SQLite 3.53.4**
(measured with `ELECTRON_RUN_AS_NODE`). The rusqlite `bundled` SQLite must be ≥ 3.53 so bm25/snippet output and
FTS5 behaviour match. A test asserts `sqlite_version()`.

## 2. Wire types (`qrtz-model`)

Rules, all validated by a scratch build (see §7):

- `#[serde(rename_all = "camelCase")]` on structs; string enums `rename_all = "kebab-case"`
  (`google-meet`, `slack-huddle`, `asr-unavailable`, `meeting-ended`, ...).
- Every zod `.optional()` is `Option<T>` + `#[serde(default, skip_serializing_if = "Option::is_none")]`, so
  absent stays absent (TS `JSON.stringify` drops `undefined`).
- Every zod `.default(x)` is `#[serde(default)]`.
- No `deny_unknown_fields` (zod strips unknown keys; serde ignores them). Same tolerance both ways.
- **Numbers.** Epoch timestamps (`*At`) are `i64` (`EpochMs`). Offsets/durations (`*Ms`) are `i64` (`Ms`) but read
  through `de_ms`, which accepts a float and rounds. TS writes `z.number()`, and nothing guarantees an integer
  (for example `recording.stopped.durationMs` from the helper). `serde_json` writes `1.0` for an `f64`, which
  would break byte-equal vectors and look odd in TS, so we never write floats for ms. `confidence` and `rms` stay `f64`.

```rust
// crates/qrtz-model/src/lib.rs (abridged; full enums as in lifecycle.ts / types.ts)
pub type EpochMs = i64;
pub type Ms = i64;

pub enum MeetingPlatform { Zoom, GoogleMeet, Teams, SlackHuddle, Webex, Other }
pub enum MeetingStatus { Recording, Processing, Done, Failed }
pub enum SegmentSource { Live, Post }

pub struct Meeting {            // = MeetingWithDuration / MeetingRow
    pub id: String, pub title: String, pub platform: MeetingPlatform,
    pub started_at: EpochMs, pub ended_at: Option<EpochMs>,
    pub participants: Vec<String>,          // default []
    pub status: MeetingStatus,
    pub audio_path: Option<String>, pub template: Option<String>, pub duration_ms: Option<Ms>,
}
pub struct Segment {
    pub id: String, pub start_ms: Ms, pub end_ms: Ms, pub text: String,
    pub speaker: Option<String>, pub speaker_name: Option<String>,
    pub source: SegmentSource, pub confidence: Option<f64>,
}
pub struct MeetingNote { pub id: String, pub at_ms: Ms, pub text: String, pub created_at: EpochMs }
pub struct MeetingSummary {
    pub summary: String, pub action_items: Vec<ActionItem>, pub qa: Vec<QaItem>,
    pub provider: String, pub template: Option<String>, pub llm_error: Option<String>,
}
pub struct SearchHit { pub meeting_id: String, pub title: String, pub started_at: EpochMs,
                       pub snippet: String, pub start_ms: Option<Ms> }
pub struct MeetingListItem { pub meeting: Meeting, pub hit: Option<SearchHit> }
pub struct MeetingDetail { pub meeting: Meeting, pub segments: Vec<Segment>,
                           pub summary: Option<MeetingSummary>, pub notes: Vec<MeetingNote> }

// ---- lifecycle (lifecycle.ts) ----
pub enum RecordingPhase { Idle, Detected, Starting, Recording, Reconnecting, Stopping, Processing, Done, Error }
pub enum StopReason { Manual, MeetingEnded, HelperLost, CrashRecovered }
pub enum Degradation { AsrUnavailable, AudioUnavailable, MicOnly, AxUnavailable, TranscriptGap }
pub struct LifecycleError { pub code: String, pub message: String, pub retryable: bool }
pub struct DetectedMeeting { pub key: String, pub title: String, pub platform: MeetingPlatform, pub app: String }
pub struct ReconnectInfo { pub attempt: u32, pub since: EpochMs, pub deadline_at: EpochMs }
pub struct RecordingState {
    pub phase: RecordingPhase,
    pub detected: Option<DetectedMeeting>, pub meeting_id: Option<String>,
    pub started_at: Option<EpochMs>, pub stopped_at: Option<EpochMs>, pub stop_reason: Option<StopReason>,
    pub degraded: Vec<Degradation>,          // default []
    pub reconnect: Option<ReconnectInfo>, pub error: Option<LifecycleError>,
    pub audio_parts: Vec<String>,            // default []
    pub part_offset_ms: Ms,                  // default 0
    pub part_offsets_ms: Option<Vec<Ms>>,    // H7 (ADR 0032): parallel to audio_parts once a part was added
    // N29/H8 (ADR 0025): all omitted when empty
    pub silent: Vec<SilentInput>,            // {source, since_ms} silent right now
    pub silent_spans: Vec<SilentSpan>,       // {source, start_ms, end_ms?} closed stretches, meeting ms, ≤ 256
    pub fallbacks: Vec<CaptureFallback>,     // {from, to: Option<String> (null), reason, at_ms}, ≤ 8
    // N35 (ADR 0028): omitted when empty
    pub device_lost: Vec<Degradation>,       // degraded flags only a failed device set; helper-restored drops them
    pub device_failures: Vec<DeviceFailure>, // {degraded, reason, at_ms}, ≤ 8; never written for a revoke
    pub lost_spans: Vec<SilentSpan>,         // N39 (ADR 0031): a source not delivered at all, ≤ 16; end_ms None = still lost
}
pub enum CaptureSource { System, Mic }
pub struct CaptureReport { pub silent_spans: Vec<SilentSpan>, pub fallbacks: Vec<CaptureFallback>,
                           pub device_failures: Vec<DeviceFailure> /* N35 */,
                           pub lost_spans: Vec<SilentSpan> /* N39 */ }
#[serde(tag = "type", rename_all = "kebab-case", rename_all_fields = "camelCase")]
pub enum LifecycleEvent {
    Detected { meeting: DetectedMeeting }, MeetingEnded { key: String },
    StartRequested { meeting_id: String, at: EpochMs, audio_path: String },
    StartAcked, StartFailed { error: LifecycleError }, Degraded { flag: Degradation, at: Option<EpochMs> },
    InputSilent { source: CaptureSource, since_ms: Ms }, InputAudible { source: CaptureSource, at: EpochMs },
    CaptureFallback { from: String, to: Option<String>, reason: String, at: EpochMs },
    CaptureDegraded { degraded: Vec<Degradation>, reason: String, at: EpochMs },   // N35, only in Recording
    HelperLost { at: EpochMs, deadline_at: EpochMs }, HelperRestored { at: EpochMs, audio_path: String },
    StopRequested { reason: StopReason, at: EpochMs }, Stopped { at: EpochMs },
    ProcessingDone, ProcessingFailed { error: LifecycleError }, Rejected { error: LifecycleError },
    ClearError, Reset,
}
pub enum StepId { Finalize, Summary, Index }
pub enum StepStatus { Pending, Running, Retrying, Done, Failed, Skipped }
pub struct StepState { pub status: StepStatus, pub attempts: u32, pub max_attempts: u32,
    pub next_retry_at: Option<EpochMs>, pub error: Option<LifecycleError>, pub fallback: Option<bool>,
    pub started_at: Option<EpochMs>, pub finished_at: Option<EpochMs> }
pub struct ActiveSpeakerEvent { pub at_ms: Ms, pub name: String }
pub struct PipelineContext { pub started_at: EpochMs, pub ended_at: EpochMs, pub duration_ms: Ms,
    pub audio_parts: Vec<String>, pub part_offsets_ms: Option<Vec<Ms>> /* H7 */,
    pub active: Vec<ActiveSpeakerEvent>, pub helper_crashed: bool }
// qrtz_lifecycle::wav (H7/N41/N44, ADRs 0032-0033): stitch_parts(parts, offsets_ms: Option<&[i64]>) pads each gap
// (gap_pad_bytes, capped at STITCH_MAX_GAP_MS = 35 000), streams, and commits via `<main>.stitched`.
pub struct PipelineSteps { pub finalize: StepState, pub summary: StepState, pub index: StepState }
pub struct PipelineState { pub meeting_id: String, pub stop_reason: StopReason, pub steps: PipelineSteps,
    pub ctx: PipelineContext, pub updated_at: EpochMs,
    pub capture: Option<CaptureReport> /* N29/H8: written at stop and R1 recovery, omitted when empty */ }
pub struct PipelineView { /* PipelineState minus ctx (so it carries capture) */ }

// ---- live transcript + views ----
pub struct LiveSegment { pub id: String, pub text: String, pub start_ms: Ms, pub end_ms: Ms, pub speaker: Option<String> }
pub struct LivePartial { pub id: String, pub text: String, pub start_ms: Ms }
pub struct LiveTranscript { pub meeting_id: String, pub rev: u64, pub segments: Vec<LiveSegment>,
                            pub partial: Option<LivePartial> /* serialized as null, not omitted */ }
pub struct LiveTranscriptDelta { pub meeting_id: String, pub base_rev: u64, pub rev: u64,
                                 pub append: Vec<LiveSegment>, pub partial: Option<LivePartial> /* null */ }
pub struct SessionView { /* every field of note/contract.ts SessionView, same optionality;
    N29: silent: Vec<SilentInput> (omitted when empty), no_audio: bool (`noAudio: true`, omitted when false);
    N35: device_lost: Vec<Degradation> (omitted when empty) */ }
pub type PermissionsStatus = BTreeMap<OsPermission, PermissionState>;
```

Note: `LiveTranscript.partial` is `.nullable()` and **not** optional, so it is `Option<_>` **without**
`skip_serializing_if`, and `None` serializes as `null`.

**TS types for Bobs.** The shell's transport is envelope-based (`qrtz_invoke`, `qrtz_subscribe`, `qrtz:event`,
see `packages/core/src/ipc/tauri-transport.ts`). So during the dual run the renderer keeps its zod types, and
the Rust types are checked _against_ zod instead of generating TS:

1. **Golden round-trip:** every sample in `spec/vectors/**` deserializes and re-serializes to the identical
   `serde_json::Value`.
2. **Schema check:** a `cargo test` validates Rust-serialized samples of every request/response/event payload this
   core produces against `contracts/ipc-contract.json` (the JSON Schema Bobs renders from zod), using the
   `jsonschema` crate.
3. **After cutover (P7):** turn on the `specta` feature on `qrtz-model` (`#[cfg_attr(feature = "specta",
   derive(specta::Type))]` is on every type from day one), export with `specta-typescript`, and flip the source of
   truth. I picked specta over ts-rs because tauri-specta can also type events if we ever move off the
   envelope. ts-rs 12 is the stable fallback if specta's RC churn bites.

## 3. `qrtz-storage`

### 3.1 Connection, migrations, threading

```rust
pub struct Migration { pub version: u32, pub name: &'static str, pub sql: &'static str }

/// Exactly the TS list, same numbers and names (v2 keeps its shipped name "memo").
pub const MIGRATIONS: &[Migration] = &[
    Migration { version: 1, name: "kv",                  sql: include_str!(".../0001_kv.sql") },
    Migration { version: 2, name: "memo",                sql: include_str!(".../0002_memo.sql") },
    Migration { version: 3, name: "segment_vectors",     sql: include_str!(".../0003_segment_vectors.sql") },
    Migration { version: 4, name: "notes_templates",     sql: include_str!(".../0004_notes_templates.sql") },
    Migration { version: 5, name: "recording_lifecycle", sql: include_str!(".../0005_recording_lifecycle.sql") },
];

pub struct MigrationReport { pub from: u32, pub to: u32, pub applied: Vec<String> /* "2_memo" */ }

/// Port of migrate(): contiguous-from-1 check, refuses a newer DB, one BEGIN IMMEDIATE tx per migration,
/// sets PRAGMA user_version inside it, rolls back and reports "<v>_<name>" on failure.
pub fn migrate(conn: &mut rusqlite::Connection, ms: &[Migration]) -> Result<MigrationReport, StorageError>;
pub fn schema_version(conn: &rusqlite::Connection) -> Result<u32, StorageError>;

pub struct Db { /* rusqlite::Connection */ }
impl Db {
    /// PRAGMA journal_mode=WAL; foreign_keys=ON; busy_timeout=5000; then migrate(MIGRATIONS).
    pub fn open(path: &Path) -> Result<Db, StorageError>;
    pub fn open_in_memory() -> Result<Db, StorageError>;
    /// Read-only, no migrate (backup import source). Checks PRAGMA integrity_check.
    pub fn open_readonly(path: &Path) -> Result<Db, StorageError>;
    pub fn health(&self) -> DbHealth;               // { ok, schemaVersion, fts5 }  (db:health)
    pub fn notes(&self) -> NoteRepo<'_>;
    pub fn lifecycle(&self) -> LifecycleRepo<'_>;
    pub fn kv(&self) -> KvRepo<'_>;                 // table exists since v1; unused by TS today
    pub fn tx<R>(&mut self, f: impl FnOnce(&rusqlite::Transaction) -> Result<R, StorageError>) -> Result<R, StorageError>;
    pub fn snapshot_to(&self, path: &Path) -> Result<(), StorageError>;   // VACUUM INTO (data.snapshot)
    pub fn import_from(&mut self, src: &Path, audio_map: &HashMap<String, PathBuf>)
        -> Result<ImportReport, StorageError>;                            // data.importDb, same merge rules
}

/// Async front door: one dedicated OS thread owns the writer connection (matches the worker's single
/// DatabaseSync). Calls are FIFO, so writes stay ordered (the TS saveChain guarantee comes for free).
#[derive(Clone)]
pub struct StorageHandle { /* mpsc::Sender<Job> */ }
impl StorageHandle {
    pub fn spawn(path: PathBuf) -> Result<StorageHandle, StorageError>;   // opens + migrates on the thread
    pub async fn call<R: Send + 'static>(
        &self, f: impl FnOnce(&mut Db) -> Result<R, StorageError> + Send + 'static,
    ) -> Result<R, StorageError>;
    pub async fn close(self) -> Result<(), StorageError>;
}
```

`call` wraps the closure in `catch_unwind`. A panic becomes `StorageError::Panicked`, and the thread keeps
serving. That is the closest in-process equivalent of F15 (worker crash: the in-flight step retries). A later
read-only connection pool for `note:list`/search under WAL is an optimisation, not part of the API.

**Sharing the SQL.** Today the SQL is inline TS strings. Plan P1 moves it into
`packages/core/src/db/sql/000N_<name>.sql`. TS imports them with Vite `?raw` (vitest and Forge-Vite both support
it; `dist-types` needs a `declare module '*.sql?raw'`), and Rust uses `include_str!`. **Byte-identical SQL
on both sides** is the guarantee that "a DB created by Electron opens in Rust and vice versa". Two more tests
on top: (a) a `sqlite_master` dump of a DB migrated by TS equals one migrated by Rust; (b) cross-open: a
fixture DB written by the TS repo (`spec/fixtures/db/v5-ts.sqlite`, generated by a vitest script) is read and
written by Rust and then re-read by TS. Schema rule from the plan: during the dual run, a migration is added to the
`.sql` folder for both sides at once, and Rust never ships a migration TS lacks.

### 3.2 `NoteRepo` (1:1 with `repo.ts`)

```rust
impl NoteRepo<'_> {
    pub fn upsert_meeting(&self, m: &Meeting) -> Result<()>;   // same ON CONFLICT column list (not template)
    pub fn set_template(&self, id: &str, template: &str) -> Result<()>;
    pub fn set_status(&self, id: &str, status: MeetingStatus) -> Result<()>;
    pub fn get_meeting(&self, id: &str) -> Result<Option<Meeting>>;
    pub fn list_meetings(&self, limit: u32, offset: u32) -> Result<Vec<Meeting>>;   // started_at DESC
    pub fn list_by_status(&self, statuses: &[MeetingStatus]) -> Result<Vec<Meeting>>; // started_at ASC
    pub fn delete_meeting(&self, id: &str) -> Result<()>;   // meetings_fts row, segments, meeting (cascade rest)

    pub fn append_segment(&self, meeting_id: &str, s: &Segment) -> Result<()>;   // idempotent upsert
    pub fn replace_segments(&self, meeting_id: &str, source: SegmentSource, segs: &[Segment]) -> Result<()>;
    pub fn get_segments(&self, meeting_id: &str) -> Result<Vec<Segment>>;        // post if any, else live
    pub fn rename_speaker(&self, meeting_id: &str, from: &str, to: &str) -> Result<u32>;

    pub fn get_notes(&self, meeting_id: &str) -> Result<Vec<MeetingNote>>;
    pub fn add_note(&self, meeting_id: &str, n: &MeetingNote) -> Result<()>;
    pub fn update_note(&self, id: &str, text: &str) -> Result<bool>;
    pub fn delete_note(&self, id: &str) -> Result<bool>;

    pub fn save_summary(&self, meeting_id: &str, s: &MeetingSummary, now: EpochMs) -> Result<()>; // + meetings_fts
    pub fn get_summary(&self, meeting_id: &str) -> Result<Option<MeetingSummary>>;

    // FTS
    pub fn search(&self, input: &str, limit: u32) -> Result<Vec<SearchHit>>;           // note:list ?query
    pub fn search_segments(&self, input: &str, limit: u32) -> Result<Vec<SegmentHit>>; // chat retrieval (OR)
    // vectors (qrtz-search consumes these)
    pub fn missing_vectors(&self, model: &str, limit: u32, meeting_id: Option<&str>) -> Result<Vec<VectorTodo>>;
    pub fn save_vectors(&self, model: &str, rows: &[VectorRow]) -> Result<()>;
    pub fn vector_sources(&self, model: &str, meeting_id: Option<&str>) -> Result<Vec<VectorSource>>;

    // worker-level compositions, so the shell never re-implements them
    pub fn list(&self, q: &ListQuery) -> Result<Vec<MeetingListItem>>;   // note.list: limit+1 paging, search path
    pub fn detail(&self, id: &str) -> Result<MeetingDetail>;             // note.get (NotFound if missing)
}
pub fn to_fts_query(input: &str) -> Option<String>;   // pure; exported for vectors
```

**FTS details to port exactly:**

- `segments_fts` is external-content (`content='segments'`) and kept in sync by the three triggers in v2.
  `meetings_fts` is a **standalone** FTS5 table keyed by the **implicit `meetings.rowid`**, written only by
  `save_summary` (delete+insert) and cleared by `delete_meeting`.
- `to_fts_query`: lowercase, then NFKC, split on `[^\p{L}\p{N}']+`, strip `'`, drop empties, keep 8,
  each word becomes `"w"*`, AND-ed. `search_segments`: lowercase + NFKC, words `[\p{L}\p{N}]{2,}`, keep 12, OR-ed.
  JS `toLowerCase` and Rust `to_lowercase` agree, including final sigma. The order lowercase→NFKC is kept.
- `search()`: title-substring hits (rank `-1e9`), then `meetings_fts` hits (`snippet(meetings_fts,-1,'[',']','…',12)`,
  limit 100), then `segments_fts` hits (`snippet(segments_fts,0,…)`, limit 500); keep the best rank per meeting with a
  strict `<`, using an **insertion-ordered** map (`IndexMap`), then a stable sort by rank. That reproduces the JS
  `Map` and stable-sort tie-breaking.
- **Lenient reads.** INTEGER-affinity columns can hold REAL if TS ever wrote a fractional number (`duration_ms`,
  `start_ms`...). Row mapping reads numbers through a `ValueRef` → `i64` helper that accepts Integer or Real and
  rounds. `participants` is JSON text with default `'[]'`. Corrupt JSON in `summaries.json` → `Corrupt` error
  (TS throws from `MeetingSummary.parse`).

### 3.3 `LifecycleRepo` (the `lifecycle.*` / `pipeline.*` worker handlers)

```rust
impl LifecycleRepo<'_> {
    pub fn save_session(&self, s: &RecordingState, now: EpochMs) -> Result<()>;  // key 'session'
    pub fn load_session(&self) -> Result<Option<RecordingState>>;                // corrupt row → Ok(None)
    pub fn save_pipeline(&self, p: &PipelineState, settled: bool) -> Result<bool>; // false if meeting deleted (F14)
    pub fn get_pipeline(&self, meeting_id: &str) -> Result<Option<PipelineState>>;  // corrupt → Ok(None)
    pub fn list_unsettled(&self) -> Result<Vec<PipelineState>>;   // settled=0 ORDER BY updated_at; skips corrupt
    pub fn orphans(&self) -> Result<Vec<Meeting>>;                 // recording|processing without a pipeline row
}
```

JSON in `lifecycle_state.json` / `pipeline_state.json` uses the §2 serde shapes, so either runtime can resume
the other's session and pipelines (the vectors in §5 prove the JSON is identical).

### 3.4 Errors

```rust
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("{what} {id} not found")]                 NotFound { what: &'static str, id: String },   // → E_NOT_FOUND
    #[error("Database schema v{db} is newer than this app (v{app})")] SchemaTooNew { db: u32, app: u32 }, // → E_VALIDATION
    #[error("Migrations must be contiguous from 1; got {got} at #{at}")] NonContiguous { got: u32, at: u32 },
    #[error("Migration {version}_{name} failed: {source}")] MigrationFailed { version: u32, name: &'static str, source: rusqlite::Error },
    #[error("corrupt: {0}")]                          Corrupt(String),           // → E_VALIDATION
    #[error(transparent)]                             Sqlite(#[from] rusqlite::Error), // BUSY/LOCKED → E_UNAVAILABLE (retryable), else E_INTERNAL
    #[error(transparent)]                             Json(#[from] serde_json::Error), // → E_INTERNAL
    #[error(transparent)]                             Io(#[from] std::io::Error),      // → E_INTERNAL
    #[error("storage thread stopped")]                Closed,                    // → E_UNAVAILABLE (retryable)
    #[error("storage task panicked: {0}")]            Panicked(String),          // → E_INTERNAL (retryable)
}
impl StorageError { pub fn code(&self) -> &'static str; pub fn retryable(&self) -> bool; }
```

The messages keep the TS wording where tests match on it (`/newer/`, `/contiguous/`, `/6_bad/`).

## 4. `qrtz-lifecycle`

### 4.1 Pure core (default features, no IO, no async)

```rust
pub fn initial_state() -> RecordingState;
pub fn is_capturing(p: RecordingPhase) -> bool;   // starting | recording | reconnecting
pub fn can_start(p: RecordingPhase) -> bool;      // idle | detected | processing | done | error
/// lifecycleReducer: Some(next) or None when illegal (caller logs + ignores).
pub fn reduce(s: &RecordingState, e: &LifecycleEvent) -> Option<RecordingState>;

pub struct Machine { /* state */ }
impl Machine {
    pub fn new(initial: RecordingState) -> Self;
    pub fn state(&self) -> &RecordingState;
    pub fn send(&mut self, e: LifecycleEvent) -> Option<&RecordingState>;   // TS send() → bool
}

pub struct RetryPolicy { pub max_attempts: u32, pub base_ms: u64, pub max_ms: u64 }
pub mod policies {
    pub const HELPER_RECONNECT_WINDOW_MS: u64 = 30_000;
    pub const START_ACK_MS: u64 = 5_000;
    pub const STOP_ACK_MS: u64 = 10_000;
    pub const APPEND_LIVE: RetryPolicy = RetryPolicy { max_attempts: 5, base_ms: 250, max_ms: 4_000 };
    pub const FINALIZE: RetryPolicy    = RetryPolicy { max_attempts: 3, base_ms: 1_000, max_ms: 16_000 };
    pub const SUMMARY: RetryPolicy     = RetryPolicy { max_attempts: 3, base_ms: 2_000, max_ms: 30_000 };
    pub const INDEX: RetryPolicy       = RetryPolicy { max_attempts: 3, base_ms: 5_000, max_ms: 60_000 };
    pub fn for_step(s: StepId) -> RetryPolicy;
}
/// round(min(max, base·2^(attempt-1)) · (0.8 + random·0.4) · scale). Same float op order as TS.
pub fn retry_delay(p: &RetryPolicy, attempt: u32, random: f64, scale: f64) -> u64;

pub enum StepPatch {
    Start { now: EpochMs },
    Success { now: EpochMs, fallback: bool },
    Skip { now: EpochMs },
    Failure { now: EpochMs, error: LifecycleError, delay_ms: u64, terminal: bool, fallback: bool },
    ManualRetry { now: EpochMs },
}
pub fn new_pipeline(meeting_id: &str, reason: StopReason, ctx: PipelineContext, now: EpochMs) -> PipelineState;
pub fn pipeline_step(p: &PipelineState, id: StepId, patch: StepPatch) -> PipelineState;
pub fn next_runnable_step(p: &PipelineState) -> Option<StepId>;
pub fn pipeline_settled(p: &PipelineState) -> bool;
pub fn pipeline_usable(p: &PipelineState) -> bool;
pub fn to_view(p: &PipelineState) -> PipelineView;
pub fn progress_label(p: Option<&PipelineState>) -> &'static str;  // "Saving transcript", "Summarizing (retrying)", ...
/// Restart rule R2: running → retrying, attempts kept.
pub fn resume_after_restart(p: PipelineState) -> PipelineState;
```

`applyTranscriptDelta` stays in TS (it's a renderer reducer). The Rust side only _produces_ deltas.

### 4.2 Runtime (feature `runtime`): the `NoteService` port

Async runtime: **tokio** (Tauri 2 already runs one; `tauri::async_runtime` is tokio). The runtime is an actor:
one task owns the `Machine`, the transcript assembler, the in-memory pipelines and timers, and serialises every
input. That removes the TS races (`startInFlight`, `pipelineRunning`) by construction. Timers use
`tokio::time`, so tests run with `#[tokio::test(start_paused = true)]` and a `Clock` that follows paused time.

```rust
pub struct NoteRuntime { /* mpsc::Sender<Cmd> to the actor */ }   // Clone + Send + Sync

pub struct RuntimeConfig {            // read per use, like deps.config().note
    pub auto_detect_meetings: bool, pub auto_record: bool, pub detect_prompt: bool,
    pub auto_summarize: bool, pub background_indexing: bool, pub default_template: String,
    pub excluded_apps: Vec<String>, pub retain_audio_days: u32,
}

pub struct RuntimeDeps {
    pub store: Arc<dyn LifecycleStore>,
    pub steps: Arc<dyn StepExecutor>,
    pub helper: Arc<dyn HelperPort>,
    pub events: Arc<dyn EventSink>,
    pub config: Arc<dyn Fn() -> RuntimeConfig + Send + Sync>,
    pub clock: Arc<dyn Clock>,
    pub ids: Arc<dyn Fn() -> String + Send + Sync>,       // uuid v4 in prod, deterministic in tests
    pub random: Arc<dyn Fn() -> f64 + Send + Sync>,       // jitter; 0.5 in vectors
    pub recordings_dir: PathBuf,
    pub retry_scale: f64,                                  // QRTZ_RETRY_SCALE
    pub fault_step: Option<StepId>,                        // e2e fault injection (debug builds only)
}

impl NoteRuntime {
    pub fn spawn(deps: RuntimeDeps) -> NoteRuntime;          // then call recover()
    pub async fn recover(&self) -> Result<RecoverReport, CoreError>;   // R1–R3 + audio prune; start() awaits it

    // requests/actions (IPC)
    pub async fn session(&self) -> SessionView;
    pub async fn start(&self, title: Option<String>) -> Result<SessionView, CoreError>;
    pub async fn stop(&self, reason: StopReason) -> Result<SessionView, CoreError>;
    pub async fn dismiss(&self) -> SessionView;
    pub async fn toggle(&self) -> Result<SessionView, CoreError>;      // global shortcut / tray
    pub async fn pause(&self, paused: Option<bool>) -> Result<SessionView, CoreError>;
    pub async fn pipeline(&self, meeting_id: &str) -> Result<Option<PipelineView>, CoreError>;
    pub async fn retry_step(&self, meeting_id: &str, step: StepId) -> Result<PipelineView, CoreError>;
    pub async fn resummarize(&self, meeting_id: &str) -> Result<MeetingSummary, CoreError>;
    pub async fn set_template(&self, meeting_id: &str, template: &str) -> Result<MeetingSummary, CoreError>;
    pub async fn live_transcript(&self, meeting_id: Option<&str>) -> Option<LiveTranscript>;
    pub async fn add_note(&self, req: AddNoteRequest) -> Result<MeetingNoteWithMeeting, CoreError>;
    pub async fn permissions(&self) -> PermissionsStatus;
    pub async fn request_permission(&self, p: OsPermission) -> Result<PermissionsStatus, CoreError>;

    // inputs from Bobs's helper supervisor
    pub fn helper_message(&self, m: HelperInput);              // parsed helper → host line (subset, below)
    pub fn helper_down(&self, reason: String);                 // Supervisor onStatus backoff (F3)
    pub fn helper_failed(&self, reason: String);               // circuit open (F4)
    pub fn set_helper_health(&self, h: HelperHealth, detail: Option<String>);  // ok|simulated|missing|down
}
```

**Injected traits** (all `#[async_trait]`, object-safe, `Send + Sync`):

```rust
/// Persistence the runtime needs. Prod impl: StorageHandle (§3); tests: in-memory map or a real :memory: Db.
pub trait LifecycleStore {
    async fn create_meeting(&self, m: Meeting) -> Result<(), StoreError>;          // note.create
    async fn delete_meeting(&self, id: &str) -> Result<(), StoreError>;            // abortStart
    async fn append_live(&self, meeting_id: &str, s: Segment) -> Result<(), StoreError>; // note.appendLive (F7)
    async fn mark_failed(&self, meeting_id: &str) -> Result<(), StoreError>;       // note.fail
    async fn add_note(&self, meeting_id: &str, text: &str, at_ms: Ms) -> Result<MeetingNote, StoreError>;
    async fn get_summary(&self, meeting_id: &str) -> Result<Option<MeetingSummary>, StoreError>;
    async fn save_session(&self, s: RecordingState) -> Result<(), StoreError>;
    async fn load_session(&self) -> Result<Option<RecordingState>, StoreError>;
    async fn save_pipeline(&self, p: PipelineState, settled: bool) -> Result<bool, StoreError>;
    async fn get_pipeline(&self, meeting_id: &str) -> Result<Option<PipelineState>, StoreError>;
    async fn list_unsettled(&self) -> Result<Vec<PipelineState>, StoreError>;
    async fn orphans(&self) -> Result<Vec<Meeting>, StoreError>;
    async fn prune_audio(&self, days: u32, now: EpochMs) -> Result<Vec<String>, StoreError>;
}

/// The three post-processing steps. Each must be idempotent (recording-lifecycle.md, pipeline table).
/// Prod impl lives with whoever owns WAV stitch/repair (storage-adjacent, mine) and LLM/embeddings (qrtz-llm/search).
pub trait StepExecutor {
    async fn finalize(&self, meeting_id: &str, ctx: &PipelineContext) -> Result<FinalizeOutcome, StepError>;
    async fn summary(&self, meeting_id: &str, mode: SummaryMode) -> Result<Option<MeetingSummary>, StepError>;
    async fn index(&self, meeting_id: &str) -> Result<IndexOutcome, StepError>;     // { indexed, skipped }
    async fn resummarize_direct(&self, meeting_id: &str, template: Option<&str>) -> Result<MeetingSummary, StepError>;
}
pub enum SummaryMode { Llm, Fallback { error: String }, Skip }
pub struct StepError { pub error: LifecycleError, pub terminal: bool }   // terminal = 401/403/400

/// Outbound to the helper. Returns Err when the helper isn't running (TS: send() throws E_UNAVAILABLE).
pub trait HelperPort { fn send(&self, cmd: HelperCommand) -> Result<(), HelperUnavailable>; }

pub enum HelperCommand {               // the subset NoteService sends; Bobs maps to host-to-helper JSON lines
    RecordingStart { audio_path: PathBuf }, RecordingStop, RecordingPause, RecordingResume,
    PermissionsCheck, PermissionsRequest { permission: OsPermission },
}
pub enum HelperInput {                 // the subset NoteService consumes; everything else stays in the bridge
    Hello, PermissionsStatus(PermissionsStatus), Participants(Vec<String>),
    MeetingDetected(DetectedMeeting), MeetingEnded { key: String },
    ActiveSpeaker { at_ms: Ms, name: String }, AudioLevel { rms: f64 }, Asr(AsrEvent),
    RecordingStarted { audio_path: String }, RecordingStopped { duration_ms: Option<Ms> },
    Error { code: String, message: String },
    CaptureSilent { source: CaptureSource, since_ms: Ms }, CaptureAudible { source: CaptureSource },   // N29
    CaptureFallback { from: String, to: Option<String>, reason: String },                             // H8
    CaptureDegraded { degraded: Vec<Degradation>, reason: String },                                   // N35
}

/// Outbound events. Bobs's shell fans these out as `qrtz:event` {event, payload, subApps}.
pub trait EventSink { fn emit(&self, e: CoreEvent); }
pub enum CoreEvent {
    Session(SessionView),                 // "note:session"   every transition synchronously; level/ASR throttled 50–150 ms
    Pipeline(PipelineView),               // "note:pipeline"
    Transcript(LiveTranscriptDelta),      // "note:transcript"
    Changed { id: String },               // "note:changed"   ("*" = everything)
    PermissionsChanged(PermissionsStatus),// "permissions:changed"
    DetectedPrompt { title: String, platform: MeetingPlatform },  // shell → OS notification (not an IPC event)
}
impl CoreEvent { pub fn ipc_name(&self) -> Option<&'static str>; }

pub trait Clock { fn now_ms(&self) -> EpochMs; fn mono_ms(&self) -> EpochMs { self.now_ms() } }  // wall + mono (N45); timers are tokio::time

#[derive(Debug, thiserror::Error)]
pub enum CoreError {   // maps 1:1 onto AppErrorJSON {code, message, retryable}
    Validation(String),   // "already recording", "already starting", "not recording"  → E_VALIDATION
    Forbidden(String),    // permission-denied start                                  → E_FORBIDDEN
    NotFound(String),     // no post-processing record                                → E_NOT_FOUND
    Timeout(String),      // start-timeout                                            → E_TIMEOUT (retryable)
    Unavailable(String),  // helper missing/not running, storage closed               → E_UNAVAILABLE (retryable)
    Internal(String),     //                                                          → E_INTERNAL
}
impl CoreError { pub fn to_app_error_json(&self) -> serde_json::Value; }
```

Why the narrow `HelperCommand`/`HelperInput` instead of the full helper protocol types: the protocol is Bobs's
(and the talk worker is changing it right now for dictation). The lifecycle only consumes the messages
`NoteService.handle()` switches on, so dictation, `ax.caption`/`ax.chat`, `log`, heartbeat and ping never reach
it, and protocol changes for talk don't touch this crate.

**Silent input and capture fallbacks (N29, H8; ADR 0025).** `capture.silent` / `capture.audible` /
`capture.fallback` map onto `HelperInput::CaptureSilent` / `CaptureAudible` / `CaptureFallback`. The UI events are
the existing ones: `note:session` carries `silent` and `noAudio` live, and `note:pipeline` carries `capture`
(`silentSpans`, `fallbacks`) once the recording stops. No new `CoreEvent` and no new channel. N35 `capture.degraded` maps
onto `HelperInput::CaptureDegraded`. `note:session` carries `deviceLost`, and `capture.deviceFailures` goes to the
meeting (ADR 0028).

### 4.3 Command ↔ IPC channel mapping

The shell keeps one `qrtz_invoke(envelope)` command (router, capability check, payload validation; Bobs). Each
channel handler is a one-liner onto these crates:

| IPC channel                                 | Rust call                                                                                                                                                                                                                      | Event(s) emitted                                                               |
| ------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------ |
| `note:session`                              | `runtime.session()`                                                                                                                                                                                                            | —                                                                              |
| `note:start` `{title?}`                     | `runtime.start(title)`                                                                                                                                                                                                         | `note:session`×n, `note:changed`                                               |
| `note:stop`                                 | `runtime.stop(StopReason::Manual)`                                                                                                                                                                                             | `note:session`, `note:pipeline`, `note:transcript` (final rev), `note:changed` |
| `note:dismiss`                              | `runtime.dismiss()`                                                                                                                                                                                                            | `note:session`                                                                 |
| `note:pause` `{paused?}`                    | `runtime.pause(paused)`                                                                                                                                                                                                        | `note:session`                                                                 |
| `note:pipeline` `{id}`                      | `runtime.pipeline(id)`                                                                                                                                                                                                         | —                                                                              |
| `note:retry-step` `{id, step}`              | `runtime.retry_step(id, step)`                                                                                                                                                                                                 | `note:pipeline`, `note:changed`                                                |
| `note:live-transcript` `{meetingId?}`       | `runtime.live_transcript(id)`                                                                                                                                                                                                  | —                                                                              |
| `note:resummarize` `{id}`                   | `runtime.resummarize(id)` (via pipeline when finalize is done, else `StepExecutor::resummarize_direct`)                                                                                                                        | `note:pipeline`, `note:changed`                                                |
| `note:set-template` `{id, template}`        | `runtime.set_template(id, t)`                                                                                                                                                                                                  | `note:changed` (shell)                                                         |
| `note:add-note` `{meetingId?, text, atMs?}` | `runtime.add_note(req)` (stamps the live offset)                                                                                                                                                                               | `note:changed` (shell)                                                         |
| `note:list` `{query?, limit?, offset?}`     | `db.notes().list(&q)`, then drop `runtime.pending_deletes()` ids (shell)                                                                                                                                                       | —                                                                              |
| `note:get` `{id}`                           | `db.notes().detail(id)`                                                                                                                                                                                                        | —                                                                              |
| `note:list-notes` `{meetingId?}`            | `db.notes().get_notes(id)` (shell resolves the missing id to the live meeting)                                                                                                                                                 | —                                                                              |
| `note:update-note` / `note:delete-note`     | `db.notes().update_note` / `delete_note`                                                                                                                                                                                       | —                                                                              |
| `note:rename-speaker`                       | `db.notes().rename_speaker`                                                                                                                                                                                                    | `note:changed` (shell)                                                         |
| `note:delete` `{id, deferMs?}`              | `runtime.schedule_delete(id, deferMs)` (refuses the meeting being recorded before any tombstone; writes the `PendingDeletes` tombstone and deletes when the window ends; `deferMs` 0/absent: `runtime.delete_meeting(id)` now) | `note:changed` (runtime)                                                       |
| `note:undo-delete` `{id}`                   | `runtime.undo_delete(id)` → `{restored}` (false: too late)                                                                                                                                                                     | `note:changed` (runtime)                                                       |
| `note:export` `{id, save}`                  | `db.notes().detail(id)`, then markdown render (`markdown.ts` port; not in these crates)                                                                                                                                        | —                                                                              |
| `db:health`                                 | `db.health()`                                                                                                                                                                                                                  | —                                                                              |
| `permissions:get` / `permissions:request`   | `runtime.permissions()` / `runtime.request_permission(p)`                                                                                                                                                                      | `permissions:changed`                                                          |
| `data:export-backup`                        | `db.snapshot_to(tmp)` + zip/manifest (shell)                                                                                                                                                                                   | `data:backup-progress` (shell)                                                 |
| `data:import-backup`                        | unzip/verify (shell) + `db.import_from(path, audio_map)`                                                                                                                                                                       | `note:changed {id:"*"}`                                                        |
| _global shortcut / tray toggle_             | `runtime.toggle()`                                                                                                                                                                                                             | as start/stop                                                                  |

**Not in these crates:** `note:chat` (qrtz-llm/search), `note:set-auto-record` / `config:*` / `secrets:*` /
`app:get-info` / `helper:status` / `hud:set-expanded` (shell), and the `talk:*` channels (talk worker; its
contract isn't on `main` yet).

## 5. Shared JSON test vectors (don't exist yet; this is the design)

No shared vectors exist today: the TS tests are inline vitest (`lifecycle.test.ts`, 11 cases;
`note-service.test.ts`, 13 integration cases; `lifecycle-steps.test.ts`, 4). Stage 2 extracts them into
`spec/vectors/lifecycle/` and adds `packages/core/src/note/lifecycle.vectors.test.ts` (vitest) and
`crates/qrtz-lifecycle/tests/vectors.rs` (cargo). Both runners read the same files. Comparisons use **exact
JSON equality**, not partial matches, so the serde shapes are tested too.

**Tier 1, pure (stage 2):**

```jsonc
// spec/vectors/lifecycle/session/reconnect-restored.json
{
  "kind": "session",
  "name": "helper loss → reconnecting → restored into a new part with an offset and a gap flag",
  "from": "lifecycle.test.ts",
  "initial": null, // null = initialRecordingState()
  "steps": [
    {
      "event": {
        "type": "start-requested",
        "meetingId": "m",
        "at": 1000,
        "audioPath": "/p0",
      },
      "expect": {
        "phase": "starting",
        "meetingId": "m",
        "startedAt": 1000,
        "degraded": [],
        "audioParts": ["/p0"],
        "partOffsetMs": 0,
      },
    },
    { "event": { "type": "start-acked" }, "expect": { "...": "full state" } },
    {
      "event": { "type": "helper-lost", "at": 2000, "deadlineAt": 32000 },
      "expect": { "...": "..." },
    },
    {
      "event": { "type": "helper-lost", "at": 2500, "deadlineAt": 32000 },
      "expect": { "...": "attempt 2, same deadline" },
    },
    {
      "event": { "type": "helper-restored", "at": 4000, "audioPath": "/p1" },
      "expect": {
        "phase": "recording",
        "meetingId": "m",
        "startedAt": 1000,
        "degraded": ["transcript-gap"],
        "audioParts": ["/p0", "/p1"],
        "partOffsetMs": 3000,
      },
    },
    { "event": { "type": "reset" }, "expect": "rejected" }, // "rejected" = reducer returned null
  ],
}
```

```jsonc
// spec/vectors/lifecycle/pipeline/summary-exhausts-then-index.json
{
  "kind": "pipeline",
  "initial": {
    "meetingId": "m",
    "stopReason": "manual",
    "now": 0,
    "ctx": {
      "startedAt": 0,
      "endedAt": 1,
      "durationMs": 1,
      "audioParts": [],
      "active": [],
      "helperCrashed": false,
    },
  },
  "steps": [
    { "step": "finalize", "patch": { "kind": "start", "now": 0 } },
    { "step": "finalize", "patch": { "kind": "success", "now": 0 } },
    { "step": "summary", "patch": { "kind": "start", "now": 1 } },
    {
      "step": "summary",
      "patch": {
        "kind": "failure",
        "now": 1,
        "error": { "code": "E", "message": "boom", "retryable": true },
        "delayMs": 100,
      },
      "expect": { "...": "full PipelineState" },
      "nextRunnable": "summary",
      "settled": false,
      "usable": false,
    },
  ],
}
```

```jsonc
// spec/vectors/lifecycle/retry-delay.json
{
  "kind": "retryDelay",
  "cases": [
    {
      "policy": { "maxAttempts": 5, "baseMs": 1000, "maxMs": 5000 },
      "attempt": 1,
      "random": 0.5,
      "scale": 1,
      "expect": 1000,
    },
    {
      "policy": { "maxAttempts": 5, "baseMs": 1000, "maxMs": 5000 },
      "attempt": 1,
      "random": 0.5,
      "scale": 0.01,
      "expect": 10,
    },
  ],
}
```

Also: `fts-query.json` (`toFtsQuery` and the chat OR-query on tricky input: NFKC ligatures, apostrophes, CJK,
emoji, >8 words) and `policies.json` (the `RETRY_POLICIES` constants, so a change on one side fails the other).
To keep it honest, the vitest runner can **regenerate** expectations (`QRTZ_VECTORS_UPDATE=1`) from the TS
reducer, so TS stays the oracle during the dual run, and cargo must match.

**Tier 2, runtime scenarios (P3, with the runtime):** a script of inputs with a virtual clock →
the expected sequence of `note:session` phases, `note:pipeline` step statuses and store writes. Inputs are
`call` (start/stop/pause/retry), `helper` (a `HelperInput`), `helperDown`, `advance` (ms), `stepResult`
(scripted `StepExecutor` outcomes) and `restart` (drop the runtime, keep the store). These cover F1–F15 and R1–R3,
one file per `note-service.test.ts` case. On the TS side, a small adapter drives `NoteService` with the mock
helper and fake timers. This is the gate for removing the TS runtime at cutover.

_Implemented (stage 4)._ `spec/scenarios/lifecycle/*.json` (51 files): `{name, about, config?, excludedApps?, random?,
retryScale?, faultStep?, script, expect}`. Script ops: `helper` (a helper → host message; `$REC` = the recordings
dir), `call` (`start`, `stop`, `toggle`, `dismiss`, `pause`, `retryStep`, `pipeline`, `addNote`, `resummarize`,
`requestPermission`; the result lands in `expect.results[label]`, `"pending"` if it never settles), `advance` (ms),
`helperDown` / `helperUp` / `helperFailed` / `helperHealth`, `steps` (queued `ok`/`fail`/`fatal`/`hang`/`skip`
outcomes for finalize/summary/index, kept across restarts; finalize `ok` runs the real finalize), `store`
(`fail`/`heal` worker methods), `wav` (write a PCM WAV, `broken` = unfinalized header), `db` (`deleteMeeting`,
`seedMeeting`), `restart` (the process dies: timers and in-flight calls are dropped, the DB and recordings stay, a
new runtime runs `recover`) and `snapshot`. `expect` = `trace` (per stream: deduped session keys, per-meeting
pipeline step strings, helper commands, transcript deltas, `note:changed`, permissions, prompts), `results` and
`snapshots` (view, live transcript, DB rows, recordings dir).

- TS oracle: `apps/desktop/src/main/note-service.scenarios.test.ts` drives the real `NoteService` and worker
  handlers on a file DB with vitest fake timers; `QRTZ_VECTORS_UPDATE=1` rewrites `expect`.
- Rust: `crates/qrtz-lifecycle/tests/scenarios.rs` replays the same files against `NoteRuntime` on a paused
  current-thread tokio runtime (`TokioClock`, `InlineDb`); `QRTZ_SCENARIO=<substring>` filters,
  `QRTZ_SCENARIO_DUMP=1` writes what Rust produced to the temp dir.
- Coverage: F1–F16, R1–R3, kill/restart while starting, recording, reconnecting, stopping, finalize running/backoff,
  summary running/backoff, index running and after done; auto-record, detection prompt/excluded apps, pause across
  a helper restart, racing starts, idempotent stop, stale acks, retry scale, fault injection, audio retention.
- Observed TS behaviour the scenarios pin (and recording-lifecycle.md words differently): after a restart a
  `retrying` step keeps its persisted `nextRetryAt`, so it runs when that passes, not at once (only `running` steps
  are reset).

**Runtime shape (as built).** `qrtz_lifecycle::runtime` (feature `runtime`; `cargo test --workspace` enables it via a
self dev-dependency): `NoteRuntime::new(RuntimeDeps)` + `recover()`. State sits behind one short-lived lock that is
never held across an `await`, and each TS method is ported line for line with its await points, so a
current-thread runtime reproduces the TS event loop. The deps are `LifecycleStore` (`Store<StorageHandle>` in the
app), `StepExecutor` (`CoreSteps`: real finalize; summary/index are stubs until qrtz-llm/qrtz-search), `HelperPort`
(`send` → bool, `restart`), `EventSink` (`CoreEvent`), `Clock`, ids, random, `retry_scale`, `fault_step` and
`default_title`. `kill()` simulates process death. Errors are `CoreError {code, message, retryable}` (AppError JSON
via `to_app_error_json`).

## 6. Ambiguities in the TS source (need a decision from Bobs or Jared)

1. **Port quirks I will copy unless told otherwise** (all covered by vectors):
   - `pipelineStep 'start'` keeps the previous `error` on a running step; only `success`/`skip`/`manual-retry` clear it.
   - `manual-retry` on `finalize` re-arms **every** later step, even `done` ones. On `summary` it re-arms `index`
     only if it isn't `done`.
   - `helper-lost` while already `reconnecting` bumps `attempt` but does **not** extend `deadlineAt`.
   - `detected` is accepted during `processing` and **replaces** the session's `meetingId`, so that meeting's
     pipeline no longer drives the session to `done`/`error` (it still finishes; only the session view stops tracking it).
   - `view()` omits `startedAt` when it is `0` (truthiness check). Harmless with real clocks, but it shows up in vectors.
2. **`meetings_fts` depends on the implicit `meetings.rowid`.** `meetings` has a TEXT primary key, so its rowid isn't
   stable under `VACUUM`. Nothing runs a plain `VACUUM` today (`VACUUM INTO` doesn't renumber the source, and
   import rebuilds through the repo), but any future `VACUUM`/compaction would silently mismatch summary search.
   Options: (a) a rule of "never VACUUM" in both runtimes; (b) migration 6 adding `meeting_id UNINDEXED` to
   `meetings_fts`, or rebuilding it from `summaries`. **Needs a decision before anyone adds compaction.** Rust will
   follow (a) until then.
3. **Number types on the wire.** I'm proposing integer ms everywhere (rounded on read). If the helper ever sends
   sub-ms floats for ASR timestamps, the Rust runtime rounds where TS kept the float (only within the live
   view; the DB already rounds). OK?
4. **`SessionView.error` (string) duplicates `errorInfo.message`.** It's kept for parity. Can we drop `error` when the
   renderer moves fully to `errorInfo`?
5. **Migration numbering ownership.** The feature proposal's consent work planned **migration 6**. The talk worker
   currently adds no tables (checked the uncommitted tree), but if talk history moves into SQLite it will also want 6. Proposal: claim numbers by committing the `.sql` file first (push-to-main makes this a lock); whoever
   rebases second takes the next number.
6. **Who owns the `StepExecutor` impl?** finalize (WAV repair/stitch + speaker fusion) is storage-adjacent and I can
   take it. summary/index need qrtz-llm/qrtz-search (plan P4), so until then, does the Tauri build run with
   `summary` as the extractive fallback only and `index` returning `skipped`?
7. **Deleting a meeting leaks its audio (confirmed).** Neither the worker `note.delete` (`repo.deleteMeeting`) nor
   the deferred-delete finalizer in `index.ts` removes `audio_path` or leftover `.p<n>.wav` parts. Rust will match TS
   (leave the files) for parity. I recommend fixing it on both sides at once: delete the WAV and its parts after the row
   is gone. **Resolved (cycle 4, ADR 0016):** `note.delete` / `LifecycleStore::delete_meeting` return the files and
   `NoteService.deleteMeeting` / `NoteRuntime::delete_meeting` remove them inside the recordings directory.
8. **Deferred deletes (tombstones), ported.** `qrtz_storage::PendingDeletes` is the Electron `PendingDeletes` file
   (`<app data>/pending-deletes.json`, a JSON id array, `.tmp` + rename; 0600 here), so a tombstone left by one build
   is completed by the other. Give `RuntimeDeps::pending_delete = Some(Arc::new(PendingDeletes::new(app_data)))`.
   `NoteRuntime::schedule_delete` / `undo_delete` / `pending_deletes` / `assert_deletable` serve `note:delete`,
   `note:undo-delete` and the `note:list` filter. Undo timers die with the process; `NoteRuntime::recover` skips
   tombstoned meetings (N17), then completes every tombstone (a failed delete keeps it for the next launch) and emits
   `Changed{"*"}` if there were any. Scenarios: `delete-undo-window`, `delete-deferred-relaunch`,
   `delete-deferred-refused-live`, `delete-deferred-fails-kept`, `N17-relaunch-skips-pending-delete` (the TS runner
   wires the real `PendingDeletes` as `index.ts` does; ops `scheduleDelete`, `undoDelete`, `pendingDelete`; snapshots
   list `tombstones` when there are any).
   **`note:list` paging with tombstones.** The shell filters pending deletes **after** the `limit+1` query, so a page
   can come back short and still look like it has more. I'll keep that unless we push the tombstone filter into
   storage (`ListQuery { exclude: Vec<String> }` is cheap to add).
9. **Toolchain pin** (see §1): rustc 1.85 on the box can't build specta rc.25. Bobs, which version does your shell use?

## 7. How this sketch was checked

- In a scratch crate outside the repo, the §2 lifecycle types (`RecordingState`, `LifecycleEvent`, `PipelineState`,
  `PipelineView`, `StepState`, `PipelineContext`) were derived with the serde attributes above. They round-tripped
  JSON produced by the **real TS reducer** (bundled from `packages/core/src/note/lifecycle.ts`: 11 events,
  12 states, 7 pipeline snapshots, 1 view), producing identical `serde_json::Value` every time, including absent
  optionals, `null`-free output, kebab-case tags and a fractional `partOffsetMs` rounded on read.
- The `specta` derive feature did **not** build on the box's rustc 1.85.1 (needs a newer toolchain), hence the pin.

## 8. Stage plan (mine)

| Stage        | Lands                                                                                                                                                        | Exit test                                                                                   |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------------ | ------------------------------------------------------------------------------------------- |
| 1 (this doc) | API sketch                                                                                                                                                   | Bobs/Jared sign off on §6                                                                   |
| 2            | root `Cargo.toml`, `qrtz-model`, shared `.sql` files (TS reads them via `?raw`), `spec/vectors/lifecycle` tier 1 + vitest runner, `qrtz-lifecycle` pure core | `npm test` and `cargo test --workspace` green, vectors pass in both                         |
| 3            | `qrtz-storage` (migrate, NoteRepo, LifecycleRepo, StorageHandle, snapshot/import)                                                                            | schema-dump equality, cross-open fixture DB both ways, FTS search parity on a seeded corpus |
| 4            | `qrtz-lifecycle` runtime + tier-2 vectors + `finalize` executor                                                                                              | F1–F15, R1–R3 scenarios pass against TS and Rust                                            |

Every stage is small commits straight to `main`, never adds GitHub Actions workflows, and keeps the Electron build untouched.

## 9. `qrtz-llm` (summary step, ADR 0042)

`LlmSteps<E: DbExec>` is the `StepExecutor` for `summary` and `resummarize`. It delegates `finalize` and `index`
to `inner`. Wire it in place of `CoreSteps`:

```rust
let core = Arc::new(CoreSteps { db: storage.clone() });
let steps = Arc::new(qrtz_llm::LlmSteps::new(
    storage.clone(),
    core,
    Arc::new(now_ms),
    Arc::new(move || llm_config_from_settings()), // read per attempt
    Arc::new(qrtz_llm::EnvOverride::process(move |n: &str| secret_store.get(n))),
));
// On-device only (no provider, no keys): qrtz_llm::LlmSteps::on_device(storage.clone(), core, Arc::new(now_ms))
```

- `LlmConfig { provider, model, base_url, local_only }` maps `note.llmProvider` (default `openrouter`),
  `note.llmModel` (empty: `anthropic/claude-haiku-5.5`, `OPENROUTER_DEFAULT_MODEL`), `QRTZ_LLM_BASE_URL` and
  `privacy.localOnly` (cloud providers become on-device; keys are never read). Keys: `SecretSource::get("llm.openrouter")`
  and the other `SECRET_NAMES`. `EnvOverride` gives `QRTZ_SECRET_*` precedence, like the Electron store.
- `summary(id, Llm)`: strict, one attempt under `SUMMARY_DEADLINE_MS` (110 s, monotonic). On success the summary is
  saved and the meeting marked done. A provider failure is returned with its retry class (5xx/429, transport, and every
  after-200 failure are retryable; other 4xx are not), and a timeout is a retryable `E_TIMEOUT` with the request
  dropped, so a late reply is never saved. With no usable provider (none, local-only, no key, or nothing to summarize)
  the extractive summary is saved. `Fallback{error}`: the extractive summary with `llmError` (≤ 500 UTF-16 units).
  `Skip`: done, no summary.
- `resummarize(id, template)`: the template is stored first. Not strict: a provider failure saves the extractive
  summary with `llmError`; a timeout saves nothing and the old summary stays. Status is unchanged.
- The extractive summary caps its lists (50 transcript + 20 note action items, 30 Q&A; N54, ADR 0048) and runs on
  `spawn_blocking`.
- The prompt lists capture gaps from `PipelineState.capture` (silent spans, plus lost spans as that source's
  silence; only both silent is "no audio at all").
- DB errors keep the storage classification (busy / full disk retryable, N13); a missing meeting is `E_NOT_FOUND`.
- Keys never appear in logs or errors: `Secret` prints `***`, and error text that echoes a key is redacted.
- Parity: `spec/vectors/llm/*.json` (prompt and exact request body, reply classification, reply parser, extractive),
  written by `packages/core/src/note/llm.vectors.test.ts` and read by `crates/qrtz-llm/tests/*.rs`, plus a mock HTTP
  server (`tests/common`).

## 9. `qrtz-backup`: Markdown export and backup zip (phase 3, done)

Owner: Bozniak. ADR 0043. Port of `markdown.ts`, `backup.ts` and the `note:export` / `data:*-backup` handlers.
All calls are synchronous over `&Db`; run them on the storage thread (`StorageHandle::call`).

```rust
// Markdown (byte-identical to TS on spec/vectors/export/markdown.json)
qrtz_backup::export_meeting(db: &Db, id: &str) -> Result<MeetingExport { filename, markdown }, StorageError> // NotFound
qrtz_backup::meeting_to_markdown(&Meeting, &[Segment], Option<&MeetingSummary>, &[MeetingNote]) -> String
qrtz_backup::export_filename(&Meeting) -> String

// Backup (progress: FnMut(Phase::{Write | Verify | Import}, done, total))
qrtz_backup::export_backup(&ExportBackup { db, target, recordings_dir, temp_dir, app_version, now_ms }, progress)
    -> Result<ExportReport { saved_to, meetings, audio_files, bytes }, StorageError>   // serde: camelCase
qrtz_backup::import_backup(&ImportBackup { db, source, recordings_dir, temp_dir, pending: Option<&PendingDeletes>,
    limits: ImportLimits::default() }, progress)
    -> Result<ImportReport { imported, skipped, audio_files }, StorageError>          // serde: camelCase
// Lower level, for tests/tools: write_backup(&WriteBackup{..}), format::{Manifest, is_safe_entry}
// qrtz-storage: Db::import_from_with(src, &mut dyn ImportHooks) (import_from is the map-based wrapper)
```

Rejections are `StorageError::Validation("Backup rejected: …")` (E_VALIDATION) with the TS wording; a corrupt DB
entry is a SQLite error ("file is not a database"); a newer schema is `Validation("backup is from a newer qrtz
(schema N)")`; a full disk is `Io` (ENOSPC) or `Sqlite(DiskFull)`.

### Wiring in qrtz-app (Bobs)

1. `crates/qrtz-app/Cargo.toml`: `qrtz-backup.workspace = true`.
2. **`note:export {id, save}`**: replace `crate::markdown` with
   `storage.call(move |db| qrtz_backup::export_meeting(db, &id)).await?`. It's pinned by the shared vectors (your
   JSON-based port isn't). Keep your clipboard (`save: false`) and `QRTZ_EXPORT_DIR` / save-dialog (`save: true`)
   code; respond `{filename, markdown, savedTo?}`. Then delete `crates/qrtz-app/src/markdown.rs`.
3. **`data:export-backup {}`**: target = `QRTZ_BACKUP_PATH`, else a save dialog defaulting to
   `<documents>/qrtz-backup-<UTC ISO[0..16] with ':' and 'T' → '-'>.zip`, filter `qrtz backup`/`zip`. Cancelled:
   `{meetings: 0, audioFiles: 0, bytes: 0}`. Otherwise
   `storage.call(move |db| export_backup(&ExportBackup { db, target: &t, recordings_dir: &dirs.recordings,
   temp_dir: &std::env::temp_dir(), app_version: <package version>, now_ms: <wall clock> }, &mut progress))`, where
   `progress` throttles to about 10/s and emits `data:backup-progress {kind: "export", phase: "write", done, total}`.
   Then emit `{kind: "export", phase: "done", done: 1, total: 1}` and respond with the `ExportReport` JSON
   (`{savedTo, meetings, audioFiles, bytes}`).
4. **`data:import-backup {}`**: source = `QRTZ_BACKUP_PATH`, else an open dialog (`zip`). Cancelled:
   `{canceled: true, imported: 0, skipped: 0, audioFiles: 0}`. Otherwise
   `storage.call(move |db| import_backup(&ImportBackup { db, source: &s, recordings_dir: &dirs.recordings,
   temp_dir: &std::env::temp_dir(), pending: Some(&pending_deletes), limits: ImportLimits::default() },
   &mut progress))` with `kind: "import"` (`Phase::Verify` → `"verify"`, `Phase::Import` → `"import"`), then
   `{kind: "import", phase: "done", done: 1, total: 1}`, then `note:changed {id: "*"}`, respond with the
   `ImportReport` JSON. Pass the **same** `Arc<PendingDeletes>` as `RuntimeDeps::pending_delete`.
5. Map errors with the existing `StorageError` → `AppError` path. Both calls hold the storage thread for the
   whole run, like the TS worker; the renderer already shows progress for that.
6. E2E: `QRTZ_BACKUP_PATH` / `QRTZ_EXPORT_DIR` drive the existing Electron specs unchanged.

## 10. `qrtz-search` (index step, backfill, chat retrieval; ADR 0049)

```rust
let embedder = qrtz_search::LazyEmbedder::none();          // full-text only; with a model: LazyEmbedder::new(loader)
let steps = Arc::new(qrtz_search::SearchSteps::new(storage.clone(), llm_steps, embedder.clone()));
// The runtime's StepExecutor is now `steps` (finalize/summary/resummarize go to LlmSteps, index here).
let backfill = qrtz_search::Backfill::new(storage.clone(), Arc::new(move |id: &str| pending.has(id)));
tokio::spawn(backfill.clone().run(embedder.clone(), Duration::from_secs(60)));   // at boot; backfill.cancel() on quit
// note:chat retrieval (answering stays with the LLM side):
let r = qrtz_search::retrieve(&storage, &embedder, &question, meeting_id).await?;  // never waits for the model
// r.ranked = Some(hybrid order) when a model was ready, else rank with qrtz_search::rank_sources(&question, &r.candidates, 24)
```

- `index(id)`: `E_NOT_FOUND` for a missing meeting. With no model, `{indexed: 0, skipped: true}`. Otherwise the
  meeting's missing lines in 64-line batches under the 280 s deadline (which includes the model load); past it a
  retryable `E_TIMEOUT` with the earlier batches kept.
- `Backfill::pass(e, max_lines)` returns `{embedded, more, cancelled}`. It goes newest meeting first, skips
  tombstoned meetings, and a model change re-embeds every line.
- Stored vectors: little-endian f32 (`vec_to_blob`), the model id in `segment_vectors.model`, written with
  `NoteRepo::save_vectors_if_current` (only lines that still have the embedded text).
- Parity: `spec/vectors/search/rank.json` and the `ranks` of `spec/stress/chat-cjk.json`.
