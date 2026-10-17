//! Cycle 22, N104 end to end through `build_app` and the scripted helper: the search model is still downloading
//! (first run) while two meetings are recorded back to back. Summary and index share one queue across meetings, so
//! an index step that waited out its 280 s deadline (three times) on the download held the second meeting's summary
//! behind it and then failed the index step. Now the first meeting's index is skipped after
//! `qrtz_search::INDEX_MODEL_WAIT_MS`, the second meeting's summary runs, nothing fails, and the backfill embeds
//! both meetings once the model is ready.
mod e2e_support;
use e2e_support::*;
use qrtz_search::embedder::LoadFuture;
use qrtz_search::{Embedder, LazyEmbedder, INDEX_MODEL_WAIT_MS};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn n104_meetings_recorded_while_the_search_model_downloads_finish_and_are_embedded_later() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    let gate = Arc::new(tokio::sync::Notify::new());
    let g = gate.clone();
    let downloading = LazyEmbedder::new(Box::new(move || -> LoadFuture {
        let g = g.clone();
        Box::pin(async move {
            g.notified().await; // the 90 MB download
            Ok(Arc::new(FakeEmbedder) as Arc<dyn Embedder>)
        })
    }));
    let a = App::launch(&data, Opts::default().standup("0.02").embedder(downloading)).await;
    a.helper_ready().await;
    a.until_phase("detected").await;
    let t0 = Instant::now();
    let (first, p1) = record_until(&a, Some("First"), |t| segments(t) > 1).await;
    assert_eq!(p1["steps"]["summary"]["status"], "done", "{p1}");
    assert_eq!(p1["steps"]["index"]["status"], "skipped", "{p1}");
    let (second, p2) = record_until(&a, Some("Second"), |t| segments(t) > 1).await;
    assert_eq!(p2["steps"]["summary"]["status"], "done", "{p2}");
    assert_eq!(p2["steps"]["index"]["status"], "skipped", "{p2}");
    // Two model waits plus recording, never a 280 s deadline.
    let took = t0.elapsed();
    assert!(
        took < Duration::from_millis(2 * INDEX_MODEL_WAIT_MS + 60_000),
        "{took:?}"
    );
    // The download finishes: the backfill embeds both meetings (nothing else is live).
    gate.notify_one();
    let db_path = data.join("qrtz.db");
    wait_for("both meetings embedded", Duration::from_secs(30), || async {
        let db = qrtz_storage::Db::open(&db_path).ok()?;
        let n = |id: &str| {
            db.notes()
                .vector_sources("fake-4d", Some(id))
                .map_or(0, |v| v.len())
        };
        (n(&first) > 0 && n(&second) > 0).then_some(())
    })
    .await;
    a.quit().await;
}
