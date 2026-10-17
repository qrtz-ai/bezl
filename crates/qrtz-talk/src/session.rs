//! One provider connection for one dictation attempt: a task that owns the socket and the codec. It reports
//! [`SessionEvent`]s through a callback and takes audio / finish / close commands. Every error string that
//! leaves it has the credential redacted.
use crate::contract::DictionaryEntry;
use crate::provider::{CodecEvent, StreamingProvider};
use crate::ws::{WsConnector, WsIn, WsOut};
use std::sync::Arc;
use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    Opened,
    /// Any server frame arrived (liveness for the stall check).
    Inbound,
    Text {
        committed: String,
        partial: String,
    },
    /// The stream ended with this final text (after `finish`).
    Final(String),
    /// The stream is unusable (connect failure, socket error, early close, provider error).
    Failed(String),
}

enum Cmd {
    Audio(Vec<u8>),
    Finish,
    Close,
}

pub struct ProviderSession {
    tx: UnboundedSender<Cmd>,
}

impl ProviderSession {
    pub fn push(&self, pcm: Vec<u8>) {
        let _ = self.tx.send(Cmd::Audio(pcm));
    }
    pub fn finish(&self) {
        let _ = self.tx.send(Cmd::Finish);
    }
    pub fn close(&self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

impl Drop for ProviderSession {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Close);
    }
}

/// Replace every occurrence of the credential (and of its Bearer form) in a message from the transport or server.
pub fn redact(msg: &str, key: Option<&str>) -> String {
    match key {
        Some(k) if k.len() >= 4 => msg.replace(k, "[redacted]"),
        _ => msg.to_owned(),
    }
}

pub fn spawn(
    connector: &dyn WsConnector,
    provider: &dyn StreamingProvider,
    key: Option<String>,
    dictionary: &[DictionaryEntry],
    on_event: Arc<dyn Fn(SessionEvent) + Send + Sync>,
) -> ProviderSession {
    let mut codec = provider.codec();
    let mut conn = connector.connect(provider.request(key.as_deref(), dictionary));
    for f in codec.hello(dictionary) {
        let _ = conn.tx.send(f);
    }
    let (tx, mut rx) = unbounded_channel::<Cmd>();
    tokio::spawn(async move {
        let fail = |e: String| on_event(SessionEvent::Failed(redact(&e, key.as_deref())));
        let mut finishing = false;
        loop {
            tokio::select! {
                cmd = rx.recv() => match cmd {
                    Some(Cmd::Audio(pcm)) => { let _ = conn.tx.send(codec.audio(&pcm)); }
                    Some(Cmd::Finish) => {
                        finishing = true;
                        for f in codec.finish() { let _ = conn.tx.send(f); }
                    }
                    Some(Cmd::Close) | None => { let _ = conn.tx.send(WsOut::Close); return; }
                },
                ev = conn.rx.recv() => match ev {
                    Some(WsIn::Open) => on_event(SessionEvent::Opened),
                    Some(WsIn::Text(t)) => {
                        on_event(SessionEvent::Inbound);
                        for e in codec.on_message(&t) {
                            match e {
                                CodecEvent::Text { committed, partial } => on_event(SessionEvent::Text { committed, partial }),
                                CodecEvent::Final(t) => {
                                    on_event(SessionEvent::Final(t));
                                    let _ = conn.tx.send(WsOut::Close);
                                    return;
                                }
                                CodecEvent::Error(e) => {
                                    fail(format!("provider error: {e}"));
                                    let _ = conn.tx.send(WsOut::Close);
                                    return;
                                }
                            }
                        }
                    }
                    Some(WsIn::Error(e)) => { fail(e); let _ = conn.tx.send(WsOut::Close); return; }
                    Some(WsIn::Closed { code, reason }) => {
                        if finishing {
                            on_event(SessionEvent::Final(codec.committed()));
                        } else {
                            fail(format!("closed {} {}", code.map(|c| c.to_string()).unwrap_or_default(), reason).trim().to_owned());
                        }
                        return;
                    }
                    None => {
                        if finishing { on_event(SessionEvent::Final(codec.committed())) } else { fail("connection lost".into()) }
                        return;
                    }
                },
            }
        }
    });
    ProviderSession { tx }
}
