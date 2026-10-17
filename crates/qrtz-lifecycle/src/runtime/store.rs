//! [`LifecycleStore`] on qrtz-storage: [`StorageHandle`] in the app, [`InlineDb`] (a `Mutex<Db>` run on the
//! caller's task) in tests and tools, so a paused tokio clock never sees a cross-thread wait.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use qrtz_model::{
    EpochMs, LifecycleError, Meeting, MeetingNote, MeetingStatus, MeetingSummary, Ms, PipelineState,
    RecordingState, Segment, SegmentSource,
};
use qrtz_storage::{Db, StorageError, StorageHandle};

use super::api::{IdGen, LifecycleStore, NoteWithMeeting, StoreResult};
use crate::js::js_round;

pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Runs a closure against the database somewhere.
pub trait DbExec: Send + Sync + 'static {
    fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> qrtz_storage::Result<R> + Send + 'static,
    ) -> BoxFut<'_, qrtz_storage::Result<R>>;
}

impl DbExec for StorageHandle {
    fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> qrtz_storage::Result<R> + Send + 'static,
    ) -> BoxFut<'_, qrtz_storage::Result<R>> {
        Box::pin(self.call(f))
    }
}

/// A database used synchronously on the calling task.
#[derive(Clone)]
pub struct InlineDb(pub Arc<Mutex<Db>>);

impl InlineDb {
    pub fn new(db: Db) -> Self {
        InlineDb(Arc::new(Mutex::new(db)))
    }
}

impl DbExec for InlineDb {
    fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> qrtz_storage::Result<R> + Send + 'static,
    ) -> BoxFut<'_, qrtz_storage::Result<R>> {
        let r = f(&mut self.0.lock().unwrap_or_else(|p| p.into_inner()));
        Box::pin(std::future::ready(r))
    }
}

/// Storage errors as the worker reported them (`AppError.from`): code, message, retryable.
pub fn lifecycle_error(e: StorageError) -> LifecycleError {
    LifecycleError {
        code: e.code().into(),
        message: e.to_string(),
        retryable: e.retryable(),
    }
}

/// The [`LifecycleStore`] over any [`DbExec`].
pub struct Store<E: DbExec>(pub E);

impl<E: DbExec> Store<E> {
    async fn run<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut Db) -> qrtz_storage::Result<R> + Send + 'static,
    ) -> StoreResult<R> {
        self.0.run(f).await.map_err(lifecycle_error)
    }
}

/// `note.pruneAudio` body (shared with the app's retention job).
pub fn prune_audio_sync(db: &Db, days: u32, now: EpochMs) -> qrtz_storage::Result<Vec<String>> {
    let mut pruned = Vec::new();
    if days == 0 {
        return Ok(pruned);
    }
    let cutoff = now - i64::from(days) * 86_400_000;
    let repo = db.notes();
    for m in repo.list_meetings(100_000, 0)? {
        let Some(path) = m.audio_path.clone() else {
            continue;
        };
        if m.status != MeetingStatus::Done || m.ended_at.unwrap_or(m.started_at) > cutoff {
            continue;
        }
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => continue,
        }
        repo.upsert_meeting(&Meeting {
            audio_path: None,
            ..m.clone()
        })?;
        pruned.push(m.id);
    }
    Ok(pruned)
}

#[async_trait]
impl<E: DbExec> LifecycleStore for Store<E> {
    async fn create_meeting(&self, m: Meeting) -> StoreResult<()> {
        self.run(move |db| {
            db.notes().upsert_meeting(&m)?;
            if let Some(t) = m.template.as_deref().filter(|t| !t.is_empty()) {
                db.notes().set_template(&m.id, t)?;
            }
            Ok(())
        })
        .await
    }
    async fn delete_meeting(&self, id: String) -> StoreResult<Vec<String>> {
        self.run(move |db| {
            let files = db.notes().meeting_files(&id)?;
            db.notes().delete_meeting(&id)?;
            Ok(files)
        })
        .await
    }
    async fn append_live(&self, meeting_id: String, s: Segment) -> StoreResult<()> {
        self.run(move |db| {
            db.notes().append_segment(
                &meeting_id,
                &Segment {
                    source: SegmentSource::Live,
                    ..s
                },
            )
        })
        .await
    }
    async fn mark_failed(&self, meeting_id: String) -> StoreResult<()> {
        self.run(move |db| db.notes().set_status(&meeting_id, MeetingStatus::Failed))
            .await
    }
    async fn add_note(
        &self,
        meeting_id: String,
        text: String,
        at_ms: Ms,
        created_at: EpochMs,
        ids: IdGen,
    ) -> StoreResult<NoteWithMeeting> {
        self.run(move |db| {
            db.notes().need(&meeting_id)?;
            let note = MeetingNote {
                id: ids(),
                at_ms: js_round(at_ms as f64).max(0),
                text,
                created_at,
            };
            db.notes().add_note(&meeting_id, &note)?;
            Ok(NoteWithMeeting { note, meeting_id })
        })
        .await
    }
    async fn get_summary(&self, meeting_id: String) -> StoreResult<Option<MeetingSummary>> {
        self.run(move |db| db.notes().get_summary(&meeting_id)).await
    }
    async fn set_template(&self, meeting_id: String, template: String) -> StoreResult<()> {
        self.run(move |db| {
            db.notes().need(&meeting_id)?;
            db.notes().set_template(&meeting_id, &template)
        })
        .await
    }
    async fn save_session(&self, s: RecordingState, now: EpochMs) -> StoreResult<()> {
        self.run(move |db| db.lifecycle().save_session(&s, now)).await
    }
    async fn load_session(&self) -> StoreResult<Option<RecordingState>> {
        self.run(|db| db.lifecycle().load_session()).await
    }
    async fn save_pipeline(&self, p: PipelineState, settled: bool) -> StoreResult<bool> {
        self.run(move |db| db.lifecycle().save_pipeline(&p, settled))
            .await
    }
    async fn get_pipeline(&self, meeting_id: String) -> StoreResult<Option<PipelineState>> {
        self.run(move |db| db.lifecycle().get_pipeline(&meeting_id)).await
    }
    async fn list_unsettled(&self) -> StoreResult<Vec<PipelineState>> {
        self.run(|db| db.lifecycle().list_unsettled()).await
    }
    async fn orphans(&self) -> StoreResult<Vec<Meeting>> {
        self.run(|db| db.lifecycle().orphans()).await
    }
    async fn prune_audio(&self, days: u32, now: EpochMs) -> StoreResult<Vec<String>> {
        self.run(move |db| prune_audio_sync(db, days, now)).await
    }
}
