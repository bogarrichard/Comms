//! A small JSON-RPC 2.0 client over line-delimited streams, which is what
//! `signal-cli jsonRpc` speaks on stdin/stdout. Generic over the transport so
//! tests can drive it with in-memory pipes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};

/// Why a call failed. The split matters for send safety: [`RpcError::NotSent`]
/// means the request provably never left; [`RpcError::NoAnswer`] means it may have.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RpcError {
    #[error("{0}")]
    NotSent(String),
    #[error("{0}")]
    NoAnswer(String),
    #[error("{message} (code {code})")]
    Remote { code: i64, message: String },
}

/// A message from the peer that is not an answer to one of our calls.
#[derive(Debug, Clone, PartialEq)]
pub struct Notification {
    pub method: String,
    pub params: Value,
}

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, RpcError>>>>>;
type Writer = Box<dyn AsyncWrite + Send + Unpin>;

pub struct RpcClient {
    writer: tokio::sync::Mutex<Writer>,
    pending: Pending,
    next_id: AtomicU64,
    closed: Arc<AtomicBool>,
}

impl RpcClient {
    /// Start reading `reader`; notifications are delivered on the returned channel,
    /// which ends when the connection does.
    pub fn new<R, W>(reader: R, writer: W) -> (Self, mpsc::UnboundedReceiver<Notification>)
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        let pending: Pending = Arc::default();
        let closed = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(read_loop(reader, pending.clone(), closed.clone(), tx));
        let client = Self {
            writer: tokio::sync::Mutex::new(Box::new(writer)),
            pending,
            next_id: AtomicU64::new(1),
            closed,
        };
        (client, rx)
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }

    pub async fn call(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, RpcError> {
        if self.is_closed() {
            return Err(RpcError::NotSent("signal-cli is not running".into()));
        }
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().expect("pending lock").insert(id, tx);

        let mut line =
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}).to_string();
        line.push('\n');
        let written = async {
            let mut w = self.writer.lock().await;
            w.write_all(line.as_bytes()).await?;
            w.flush().await
        }
        .await;
        if let Err(e) = written {
            self.pending.lock().expect("pending lock").remove(&id);
            // A line without its newline is never executed by the peer.
            return Err(RpcError::NotSent(format!("writing to signal-cli: {e}")));
        }

        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(answer)) => answer,
            Ok(Err(_)) => Err(RpcError::NoAnswer("signal-cli stopped".into())),
            Err(_) => {
                self.pending.lock().expect("pending lock").remove(&id);
                Err(RpcError::NoAnswer(
                    "timed out waiting for signal-cli".into(),
                ))
            }
        }
    }
}

async fn read_loop<R: AsyncRead + Unpin>(
    reader: R,
    pending: Pending,
    closed: Arc<AtomicBool>,
    notifications: mpsc::UnboundedSender<Notification>,
) {
    let mut lines = BufReader::new(reader).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(l)) => l,
            Ok(None) => break,
            Err(e) => {
                tracing::warn!("reading from signal-cli failed: {e}");
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            // Never log the line: it may hold message text.
            tracing::debug!("ignoring a non-JSON line from signal-cli");
            continue;
        };
        if let Some(id) = value.get("id").and_then(Value::as_u64)
            && (value.get("result").is_some() || value.get("error").is_some())
        {
            let answer = match value.get("error") {
                Some(err) => Err(RpcError::Remote {
                    code: err.get("code").and_then(Value::as_i64).unwrap_or(0),
                    message: err
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown error")
                        .to_owned(),
                }),
                None => Ok(value.get("result").cloned().unwrap_or(Value::Null)),
            };
            if let Some(tx) = pending.lock().expect("pending lock").remove(&id) {
                let _ = tx.send(answer);
            }
        } else if let Some(method) = value.get("method").and_then(Value::as_str) {
            let _ = notifications.send(Notification {
                method: method.to_owned(),
                params: value.get("params").cloned().unwrap_or(Value::Null),
            });
        }
    }
    closed.store(true, Ordering::SeqCst);
    let waiting: Vec<_> = pending.lock().expect("pending lock").drain().collect();
    for (_, tx) in waiting {
        let _ = tx.send(Err(RpcError::NoAnswer(
            "signal-cli closed the connection".into(),
        )));
    }
}
