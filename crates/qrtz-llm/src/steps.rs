//! What a summary is built from: [`load_sync`] reads a meeting's title, segments, notes, template and capture record
//! (`captureOf`), and [`Loaded::extractive`] is the extractive summary of it. The summary step itself is
//! qrtz-app's `SummarySteps` (ADR 0075: on this Mac only, no network); the cloud `LlmSteps` went with ADR 0076.
use qrtz_model::{MeetingNote, MeetingSummary, Segment};
use qrtz_storage::Db;

use crate::extractive::extractive_summary;
use crate::prompt::SummaryOptions;

/// What the summary is built from.
pub struct Loaded {
    pub id: String,
    pub title: String,
    pub segments: Vec<Segment>,
    /// Notes, template and the capture record.
    pub opts: SummaryOptions,
}

impl Loaded {
    pub fn extractive_sync(&self) -> MeetingSummary {
        extractive_summary(
            &self.segments,
            None,
            &self.opts.notes,
            self.opts.template.as_deref(),
        )
    }
    /// On a blocking thread, not the runtime's worker (N54: linear and capped, but a 3 h meeting is still work).
    pub async fn extractive(&self) -> MeetingSummary {
        let (segs, notes, tpl) = (
            self.segments.clone(),
            self.opts.notes.clone(),
            self.opts.template.clone(),
        );
        tokio::task::spawn_blocking(move || extractive_summary(&segs, None, &notes, tpl.as_deref()))
            .await
            .expect("extractive summary panicked")
    }
}

/// `need(id)`, its segments, notes, template and capture record (`captureOf`). With `set_template`, the template is stored first
/// (`note.resummarize {template}`).
pub fn load_sync(db: &Db, id: &str, set_template: Option<&str>) -> qrtz_storage::Result<Loaded> {
    let repo = db.notes();
    let mut m = repo.need(id)?;
    if let Some(t) = set_template.filter(|t| !t.is_empty()) {
        repo.set_template(id, t)?;
        m.template = Some(t.to_string());
    }
    let notes: Vec<MeetingNote> = repo.get_notes(id)?;
    let mut opts = SummaryOptions {
        notes,
        template: m.template.clone().filter(|t| !t.is_empty()),
        ..Default::default()
    };
    // `captureOf`: an unreadable pipeline row is no capture record.
    if let Ok(Some(p)) = db.lifecycle().get_pipeline(id) {
        if let Some(c) = p.capture {
            if !c.silent_spans.is_empty() || !c.lost_spans.is_empty() {
                let d = m.duration_ms.filter(|d| *d != 0).unwrap_or(p.ctx.duration_ms);
                opts.duration_ms = (d != 0).then_some(d);
                opts.silent_spans = c.silent_spans;
                opts.lost_spans = c.lost_spans;
            }
        }
    }
    Ok(Loaded {
        id: id.to_string(),
        title: m.title,
        segments: repo.get_segments(id)?,
        opts,
    })
}
