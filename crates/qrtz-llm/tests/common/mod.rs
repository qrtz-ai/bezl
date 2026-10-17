//! A local mock HTTP/1.1 server (never the real network): each connection gets the next scripted reply.
#![allow(dead_code)]
use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

#[derive(Clone, Debug)]
pub enum Reply {
    Full {
        status: u16,
        content_type: String,
        body: String,
    },
    /// A 200 that declares the whole body, sends `sent` bytes of it, then drops the connection (a reset mid-body).
    Partial { body: String, sent: usize },
    /// Waits, then replies.
    Delay(Duration, Box<Reply>),
    /// Never answers.
    Hang,
    /// A redirect to `location`.
    Redirect { status: u16, location: String },
}

impl Reply {
    pub fn json(status: u16, body: impl Into<String>) -> Reply {
        Reply::Full {
            status,
            content_type: "application/json".into(),
            body: body.into(),
        }
    }
    /// A chat completion whose message content is `text`.
    pub fn content(text: &str) -> Reply {
        Reply::json(
            200,
            serde_json::json!({"choices": [{"message": {"content": text}}]}).to_string(),
        )
    }
}

#[derive(Clone, Debug)]
pub struct Seen {
    pub method: String,
    pub path: String,
    /// Lower-cased names.
    pub headers: BTreeMap<String, String>,
    pub body: String,
}

pub struct Mock {
    pub base_url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
    replies: Arc<Mutex<VecDeque<Reply>>>,
}

async fn serve(
    mut sock: tokio::net::TcpStream,
    seen: Arc<Mutex<Vec<Seen>>>,
    replies: Arc<Mutex<VecDeque<Reply>>>,
) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let head_end = loop {
        let n = sock.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let first: Vec<&str> = lines.next().unwrap().split(' ').collect();
    let mut headers = BTreeMap::new();
    for l in lines.filter(|l| !l.is_empty()) {
        let (k, v) = l.split_once(':').unwrap();
        headers.insert(k.trim().to_lowercase(), v.trim().to_string());
    }
    let len: usize = headers
        .get("content-length")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);
    while buf.len() < head_end + len {
        let n = sock.read(&mut tmp).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    seen.lock().unwrap().push(Seen {
        method: first[0].into(),
        path: first[1].into(),
        headers,
        body: String::from_utf8_lossy(&buf[head_end..]).to_string(),
    });
    let mut reply = replies
        .lock()
        .unwrap()
        .pop_front()
        .unwrap_or(Reply::json(500, "{}"));
    loop {
        match reply {
            Reply::Delay(d, r) => {
                tokio::time::sleep(d).await;
                reply = *r;
            }
            Reply::Hang => {
                tokio::time::sleep(Duration::from_secs(3600)).await;
                return;
            }
            Reply::Full {
                status,
                content_type,
                body,
            } => {
                let out = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(out.as_bytes()).await;
                let _ = sock.shutdown().await;
                return;
            }
            Reply::Redirect { status, location } => {
                let out = format!(
                    "HTTP/1.1 {status} X\r\nlocation: {location}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                );
                let _ = sock.write_all(out.as_bytes()).await;
                let _ = sock.shutdown().await;
                return;
            }
            Reply::Partial { body, sent } => {
                let out = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(out.as_bytes()).await;
                let _ = sock.write_all(&body.as_bytes()[..sent]).await;
                let _ = sock.flush().await;
                tokio::time::sleep(Duration::from_millis(20)).await;
                return; // dropped: the client sees the body cut short
            }
        }
    }
}

impl Mock {
    pub async fn start() -> Mock {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base_url = format!("http://{}", l.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let replies = Arc::new(Mutex::new(VecDeque::new()));
        let (s2, r2) = (seen.clone(), replies.clone());
        tokio::spawn(async move {
            while let Ok((sock, _)) = l.accept().await {
                tokio::spawn(serve(sock, s2.clone(), r2.clone()));
            }
        });
        Mock {
            base_url,
            seen,
            replies,
        }
    }
    pub fn push(&self, r: Reply) {
        self.replies.lock().unwrap().push_back(r);
    }
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }
}
