//! QRT-297: a package host that redirects (GitHub's release downloads go to their asset CDN). A downloader made with
//! `with_redirect_hosts` follows a redirect only to https on a listed host; anything else fails the request.
use qrtz_speech::download::{DownloadControl, Downloader};
use qrtz_speech::testkit::Server;

#[tokio::test]
async fn a_redirect_off_the_pinned_hosts_fails_closed() {
    let a = Server::start().await;
    let b = Server::start().await;
    b.put("asset", b"payload".to_vec());
    a.faults
        .lock()
        .unwrap()
        .redirect
        .insert("v1.timestamp.json".into(), format!("{}/asset", b.base));

    // Listed host, but plain http: refused (the CDN is https only).
    let pinned = Downloader::with_redirect_hosts(&a.base, &["127.0.0.1".into()]);
    let e = pinned.fetch_meta("v1.timestamp.json").await.unwrap_err();
    assert!(e.to_string().contains("redirect"), "{e}");
    let e = pinned
        .fetch_range("v1.timestamp.json", 0, 3, &DownloadControl::default())
        .await
        .unwrap_err();
    assert!(e.to_string().contains("redirect"), "{e}");

    // A host that isn't listed: refused.
    let other = Downloader::with_redirect_hosts(&a.base, &["release-assets.githubusercontent.com".into()]);
    assert!(other.fetch_meta("v1.timestamp.json").await.is_err());
    assert_eq!(
        b.faults.lock().unwrap().hits.get("asset"),
        None,
        "the redirect target was never asked"
    );

    // The plain downloader (setup.rs's speech bundle) keeps its behaviour.
    let plain = Downloader::new(&a.base);
    assert_eq!(
        plain.fetch_meta("v1.timestamp.json").await.unwrap().unwrap(),
        b"payload"
    );
}
