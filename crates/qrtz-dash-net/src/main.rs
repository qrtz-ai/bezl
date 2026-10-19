//! `qrtz-dash-net`: Agent Dash's network helper (ADR 0231 amendment 1). Spawned by qrtz-app only when dash is on
//! (debug builds) and the user opted in; reads one `dash:*` call per stdin line, writes one answer per stdout
//! line, and exits when stdin closes. Nothing else: no arguments, no files, no other host.
use qrtz_dash_net::{platform_store, Helper, Https, MAX_LINE};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let helper = Helper {
        transport: Box::new(Https::default()),
        store: platform_store(),
    };
    let mut lines = BufReader::with_capacity(MAX_LINE, tokio::io::stdin()).lines();
    let mut out = tokio::io::stdout();
    while let Ok(Some(line)) = lines.next_line().await {
        let mut reply = helper.handle_line(&line).await.to_string();
        reply.push('\n');
        if out.write_all(reply.as_bytes()).await.is_err() || out.flush().await.is_err() {
            break;
        }
    }
}
