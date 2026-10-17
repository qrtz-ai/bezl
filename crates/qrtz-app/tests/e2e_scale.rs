//! Phase 5 ports of the scale rows (docs/e2e-parity.md rows 11, 21, 33; row 13's backend boot budget is in
//! e2e_shell.rs). Electron seeded with QRTZ_DEV_SEED=large|many; the Tauri build has no dev seed, so the tests
//! seed the same data (`dev.seedLarge` / `dev.seedMany` shapes, e2e_support::seed_large / seed_many) into the
//! data dir before the launch. Budgets are the Electron spec's, measured at the IPC boundary.
mod e2e_support;
use e2e_support::*;
use serde_json::{json, Value};
use std::time::{Duration, Instant};

fn len(v: &Value) -> usize {
    v.as_array().map_or(0, Vec::len)
}

/// A page as the renderer shows it: `note:list` returns up to `limit + 1` rows, the extra one means "show more".
fn shown(v: &Value, limit: usize) -> (Vec<Value>, bool) {
    let rows = v.as_array().cloned().unwrap_or_default();
    let more = rows.len() > limit;
    (rows.into_iter().take(limit).collect(), more)
}

/// large-a11y: a 2-hour meeting (1500 segments): open (detail, summary and the whole transcript: one `note:get`
/// payload) < 3 s, search < 2 s. Rendering the 1500 rows (Electron's separate transcript budget) is renderer work.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_hour_meeting_list_detail_transcript_and_search_stay_fast() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    seed_large(&data, 120);
    let a = App::launch(&data, Opts::default()).await;
    let list = a.note("note:list", json!({})).await;
    assert_eq!(len(&list), 1, "{list}");
    assert!(
        list[0]["meeting"]["title"]
            .as_str()
            .unwrap()
            .starts_with("All-hands"),
        "{list}"
    );

    let t = Instant::now();
    let d = a.note("note:get", json!({"id": LARGE_ID})).await;
    assert!(d["summary"]["summary"].is_string(), "summary shown");
    let segs = d["segments"].as_array().unwrap();
    assert_eq!(segs.len(), 1500);
    let open = t.elapsed();
    // Seek near the end: segment 1400 is addressable by its start time.
    assert_eq!(segs[1400]["startMs"], 1400 * 4800);

    let t = Instant::now();
    let hits = wait("search hit", || async {
        let h = a.note("note:list", json!({"query": "migration"})).await;
        (len(&h) > 0).then_some(h)
    })
    .await;
    let search = t.elapsed();
    assert_eq!(hits[0]["meeting"]["id"], LARGE_ID, "{hits}");
    assert!(
        hits[0]["hit"]["snippet"]
            .as_str()
            .unwrap_or("")
            .contains("[migration]"),
        "{hits}"
    );
    eprintln!(
        "{}",
        json!({"openMs": open.as_millis() as u64, "searchMs": search.as_millis() as u64})
    );
    assert!(open < Duration::from_secs(3), "open {open:?}");
    assert!(search < Duration::from_secs(2), "search {search:?}");
    a.quit().await;
}

/// chat: search results are paged (500 meetings all match "roadmap": 100 per page, the offset gives the next 100).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn search_results_are_paged() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    seed_many(&data, 500);
    let a = App::launch(&data, Opts::default()).await;
    let (list, more) = shown(&a.note("note:list", json!({"limit": 100})).await, 100);
    assert_eq!((list.len(), more), (100, true));
    let (p1, more) = shown(
        &a.note(
            "note:list",
            json!({"query": "roadmap", "limit": 100, "offset": 0}),
        )
        .await,
        100,
    );
    assert_eq!((p1.len(), more), (100, true), "100 snippets and a show-more");
    assert!(p1
        .iter()
        .all(|h| h["hit"]["snippet"].as_str().unwrap_or("").contains("[roadmap]")));
    let (p2, more) = shown(
        &a.note(
            "note:list",
            json!({"query": "roadmap", "limit": 100, "offset": 100}),
        )
        .await,
        100,
    );
    assert_eq!((p2.len(), more), (100, true), "show more → 200 snippets");
    let ids = |p: &[Value]| -> std::collections::BTreeSet<String> {
        p.iter()
            .map(|h| h["meeting"]["id"].as_str().unwrap().to_owned())
            .collect()
    };
    let (a1, a2) = (ids(&p1), ids(&p2));
    assert_eq!(a1.len() + a2.len(), 200, "pages are disjoint");
    assert!(a1.is_disjoint(&a2));
    a.quit().await;
}

/// states: hundreds of meetings: pages of 100, the next page < 2 s, the last page ends, the oldest is "Sync #1".
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hundreds_of_meetings_paged_list_stays_fast() {
    let tmp = tempfile::tempdir().unwrap();
    let data = tmp.path().join("data");
    seed_many(&data, 500);
    let a = App::launch(&data, Opts::default()).await;
    let page = |offset: u64| {
        let a = &a;
        async move {
            shown(
                &a.note("note:list", json!({"limit": 100, "offset": offset})).await,
                100,
            )
        }
    };
    let (first, has_more) = page(0).await;
    assert_eq!((first.len(), has_more), (100, true));
    assert_eq!(first[0]["meeting"]["title"], "Sync #500", "newest first");
    let t = Instant::now();
    assert_eq!(page(100).await.0.len(), 100);
    let more = t.elapsed();
    let mut all = 200;
    for off in [200, 300, 400] {
        all += page(off).await.0.len();
    }
    assert_eq!(all, 500);
    // The last page has no "show more".
    let (last, has_more) = page(400).await;
    assert!(!has_more, "no show-more after 500");
    assert_eq!(page(500).await.0.len(), 0);
    assert_eq!(last[99]["meeting"]["title"], "Sync #1", "{}", last[99]);
    let d = a.note("note:get", json!({"id": last[99]["meeting"]["id"]})).await;
    assert_eq!(d["meeting"]["title"], "Sync #1");
    eprintln!("{}", json!({"moreMs": more.as_millis() as u64}));
    assert!(more < Duration::from_secs(2), "next page {more:?}");
    a.quit().await;
}
