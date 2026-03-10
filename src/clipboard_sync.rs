use input_event::ClipboardFormat;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::clipboard::{ClipboardNotification, ClipboardWatcher};

const CLIPBOARD_PORT_OFFSET: u16 = 1; // base_port + 1

/// Wire protocol message for clipboard sync
#[derive(Debug, Clone, Serialize, Deserialize)]
enum ClipboardMessage {
    /// Notify peer that clipboard changed
    Changed {
        formats: Vec<ClipboardFormat>,
        size_hint: u64,
    },
    /// Request data for a format
    Request { format: ClipboardFormat },
    /// Data response
    Data {
        format: ClipboardFormat,
        data: Vec<u8>,
    },
}

/// Events from the clipboard sync layer to the service
#[derive(Debug)]
pub enum ClipboardSyncEvent {
    /// Remote clipboard changed (text available for pull)
    RemoteChanged {
        peer: SocketAddr,
        formats: Vec<ClipboardFormat>,
    },
    /// Received clipboard data from remote
    RemoteData { text: String },
}

pub struct ClipboardSync {
    event_rx: mpsc::Receiver<ClipboardSyncEvent>,
}

impl ClipboardSync {
    pub fn new(base_port: u16) -> Self {
        let (event_tx, event_rx) = mpsc::channel(16);
        let clipboard_port = base_port + CLIPBOARD_PORT_OFFSET;

        tokio::task::spawn_local(run_clipboard_server(clipboard_port, event_tx));

        Self { event_rx }
    }

    pub async fn next_event(&mut self) -> Option<ClipboardSyncEvent> {
        self.event_rx.recv().await
    }
}

async fn run_clipboard_server(port: u16, event_tx: mpsc::Sender<ClipboardSyncEvent>) {
    let listener = match TcpListener::bind(("0.0.0.0", port)).await {
        Ok(l) => {
            log::info!("clipboard sync listening on port {port}");
            l
        }
        Err(e) => {
            log::warn!("failed to start clipboard sync: {e}");
            return;
        }
    };

    // Start clipboard watcher
    let mut watcher = ClipboardWatcher::new(Duration::from_millis(500));

    loop {
        tokio::select! {
            accept = listener.accept() => {
                match accept {
                    Ok((stream, addr)) => {
                        log::info!("clipboard peer connected: {addr}");
                        let etx = event_tx.clone();
                        tokio::task::spawn_local(handle_clipboard_peer(stream, addr, etx));
                    }
                    Err(e) => log::warn!("clipboard accept error: {e}"),
                }
            }
            notification = watcher.next() => {
                if let Some(ClipboardNotification::Changed { formats, .. }) = notification {
                    log::debug!("local clipboard changed: {formats:?}");
                    // TODO: broadcast to connected peers
                }
            }
        }
    }
}

async fn handle_clipboard_peer(
    mut stream: TcpStream,
    addr: SocketAddr,
    event_tx: mpsc::Sender<ClipboardSyncEvent>,
) {
    let mut buf = vec![0u8; 64 * 1024];
    #[allow(clippy::while_let_loop)]
    loop {
        // Read length-prefixed JSON messages
        let len = match stream.read_u32().await {
            Ok(l) => l as usize,
            Err(_) => break,
        };
        if len > buf.len() {
            log::warn!("clipboard message too large: {len}");
            break;
        }
        if stream.read_exact(&mut buf[..len]).await.is_err() {
            break;
        }
        let msg: ClipboardMessage = match serde_json::from_slice(&buf[..len]) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("invalid clipboard message: {e}");
                continue;
            }
        };

        match msg {
            ClipboardMessage::Changed { formats, .. } => {
                log::debug!("remote clipboard changed from {addr}: {formats:?}");
                let _ = event_tx
                    .send(ClipboardSyncEvent::RemoteChanged {
                        peer: addr,
                        formats,
                    })
                    .await;
            }
            ClipboardMessage::Data {
                format: ClipboardFormat::Text,
                data,
            } => {
                if let Ok(text) = String::from_utf8(data) {
                    log::debug!("received clipboard text from {addr}: {} bytes", text.len());
                    let _ = event_tx.send(ClipboardSyncEvent::RemoteData { text }).await;
                }
            }
            ClipboardMessage::Request { format } => {
                log::debug!("clipboard data request from {addr}: {format:?}");
                let provider = crate::clipboard::platform_clipboard();
                if format == ClipboardFormat::Text {
                    if let Some(text) = provider.get_text() {
                        let response = ClipboardMessage::Data {
                            format: ClipboardFormat::Text,
                            data: text.into_bytes(),
                        };
                        let json = serde_json::to_vec(&response).unwrap_or_default();
                        let _ = stream.write_u32(json.len() as u32).await;
                        let _ = stream.write_all(&json).await;
                    }
                }
            }
            _ => {}
        }
    }
    log::info!("clipboard peer disconnected: {addr}");
}

/// Send a clipboard message to a peer
async fn _send_clipboard_message(addr: SocketAddr, port: u16, msg: &ClipboardMessage) {
    let clipboard_addr = SocketAddr::new(addr.ip(), port + CLIPBOARD_PORT_OFFSET);
    match TcpStream::connect(clipboard_addr).await {
        Ok(mut stream) => {
            let json = serde_json::to_vec(msg).unwrap_or_default();
            let _ = stream.write_u32(json.len() as u32).await;
            let _ = stream.write_all(&json).await;
        }
        Err(e) => {
            log::debug!("clipboard connect to {clipboard_addr} failed: {e}");
        }
    }
}
