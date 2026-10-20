//! `qrtz-dash-net`: Agent Dash's network helper (ADR 0231 amendment 1). Spawned by qrtz-app only when dash is on
//! (debug builds) and the user opted in; reads one `dash:*` call per stdin line, writes one answer per stdout
//! line, and exits when stdin closes. Nothing else: no arguments, no files, no other host.
#![forbid(unsafe_code)]
use qrtz_dash_net::{
    line_too_long, platform_store, read_line_capped, Helper, Https, Line, BUNDLE_MARKER, MAX_LINE,
};
use tokio::io::{AsyncWriteExt, BufReader};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Keeps the marker in the binary for verify-bundle.sh (M5).
    std::hint::black_box(BUNDLE_MARKER.as_bytes());
    // A dev origin override that is set but not http://127.0.0.1:<port>: refuse to run (debug + dev-local only).
    if !qrtz_dash_net::origin_config_ok() {
        eprintln!(
            "qrtz-dash-net: refusing to start: {} is set, but this build only accepts it as http://127.0.0.1:<port> \
             in a debug build with the dev-local feature",
            qrtz_dash_net::DEV_ORIGIN_VAR
        );
        std::process::exit(2);
    }
    let Ok(https) = Https::new() else {
        std::process::exit(1);
    };
    let helper = Helper {
        transport: Box::new(https),
        store: platform_store(),
    };
    let mut input = BufReader::new(tokio::io::stdin());
    let mut out = tokio::io::stdout();
    loop {
        let reply = match read_line_capped(&mut input, MAX_LINE).await {
            Ok(Line::Ok(b)) => {
                // The line may carry the setup token: both copies are zeroed when dropped (Bozniak L).
                let b = zeroize::Zeroizing::new(b);
                let line = zeroize::Zeroizing::new(String::from_utf8_lossy(&b).into_owned());
                helper.handle_line(&line).await
            }
            // Over the cap: refused; the rest of it was drained, never held.
            Ok(Line::TooLong) => line_too_long(),
            Ok(Line::Eof) | Err(_) => break,
        };
        let mut reply = reply.to_string();
        reply.push('\n');
        if out.write_all(reply.as_bytes()).await.is_err() || out.flush().await.is_err() {
            break;
        }
    }
}
