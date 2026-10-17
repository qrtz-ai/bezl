//! The injected WebSocket transport (the TS `WsFactory`). A connection is two channels: frames out, events in.
//! Contract for every connector: frames sent before `Open` are buffered and flushed once the socket opens (TS
//! `socketBase`), and the event stream ends with exactly one `Closed` (after an `Error`, too).
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

/// What to connect to. `Debug` never prints header values or subprotocols: they carry API keys.
#[derive(Clone, PartialEq)]
pub struct WsRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    /// `Sec-WebSocket-Protocol` values (Deepgram takes its key as the `token` subprotocol).
    pub protocols: Vec<String>,
}

impl std::fmt::Debug for WsRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WsRequest")
            .field("url", &self.url)
            .field(
                "headers",
                &self.headers.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
            )
            .field("protocols", &self.protocols.len())
            .finish()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum WsIn {
    Open,
    Text(String),
    Error(String),
    Closed { code: Option<u16>, reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum WsOut {
    Text(String),
    Binary(Vec<u8>),
    Close,
}

pub struct WsConn {
    pub tx: UnboundedSender<WsOut>,
    pub rx: UnboundedReceiver<WsIn>,
}

pub trait WsConnector: Send + Sync {
    fn connect(&self, req: WsRequest) -> WsConn;
}

#[cfg(feature = "net")]
pub use net::TungsteniteConnector;

#[cfg(feature = "net")]
mod net {
    use super::*;
    use futures_util::{SinkExt, StreamExt};
    use tokio::sync::mpsc::unbounded_channel;
    use tokio_tungstenite::tungstenite::{client::IntoClientRequest, http::HeaderValue, Message};

    /// The production connector: tokio-tungstenite over rustls (webpki roots). Must be called inside tokio.
    pub struct TungsteniteConnector;

    impl WsConnector for TungsteniteConnector {
        fn connect(&self, req: WsRequest) -> WsConn {
            let (out_tx, mut out_rx) = unbounded_channel::<WsOut>();
            let (in_tx, in_rx) = unbounded_channel::<WsIn>();
            let request = req.url.as_str().into_client_request().map(|mut r| {
                for (k, v) in &req.headers {
                    if let (Ok(k), Ok(v)) = (
                        tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(k.as_bytes()),
                        HeaderValue::from_str(v),
                    ) {
                        r.headers_mut().insert(k, v);
                    }
                }
                if !req.protocols.is_empty() {
                    if let Ok(v) = HeaderValue::from_str(&req.protocols.join(", ")) {
                        r.headers_mut().insert("sec-websocket-protocol", v);
                    }
                }
                r
            });
            tokio::spawn(async move {
                let closed = |code: u16, reason: &str| WsIn::Closed {
                    code: Some(code),
                    reason: reason.into(),
                };
                let request = match request {
                    Ok(r) => r,
                    Err(_) => {
                        let _ = in_tx.send(WsIn::Error("invalid provider url".into()));
                        let _ = in_tx.send(closed(1006, "invalid url"));
                        return;
                    }
                };
                let mut ws = match tokio_tungstenite::connect_async(request).await {
                    Ok((ws, _)) => ws,
                    Err(e) => {
                        // Connect errors name the host at most; headers are never echoed.
                        let _ = in_tx.send(WsIn::Error(format!("connect failed: {e}")));
                        let _ = in_tx.send(closed(1006, "connect failed"));
                        return;
                    }
                };
                let _ = in_tx.send(WsIn::Open);
                loop {
                    tokio::select! {
                        out = out_rx.recv() => {
                            let r = match out {
                                Some(WsOut::Text(t)) => ws.send(Message::text(t)).await,
                                Some(WsOut::Binary(b)) => ws.send(Message::binary(b)).await,
                                Some(WsOut::Close) | None => {
                                    let _ = ws.close(None).await;
                                    let _ = in_tx.send(closed(1000, ""));
                                    return;
                                }
                            };
                            if let Err(e) = r {
                                let _ = in_tx.send(WsIn::Error(format!("send failed: {e}")));
                                break;
                            }
                        }
                        msg = ws.next() => match msg {
                            Some(Ok(Message::Text(t))) => { let _ = in_tx.send(WsIn::Text(t.to_string())); }
                            Some(Ok(Message::Close(f))) => {
                                let _ = in_tx.send(WsIn::Closed {
                                    code: f.as_ref().map(|f| u16::from(f.code)),
                                    reason: f.map(|f| f.reason.to_string()).unwrap_or_default(),
                                });
                                return;
                            }
                            Some(Ok(_)) => {}
                            Some(Err(e)) => { let _ = in_tx.send(WsIn::Error(format!("receive failed: {e}"))); break }
                            None => break,
                        }
                    }
                }
                let _ = in_tx.send(closed(1006, "connection lost"));
            });
            WsConn {
                tx: out_tx,
                rx: in_rx,
            }
        }
    }
}
