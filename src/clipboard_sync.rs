use input_event::ClipboardFormat;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::clipboard::{ClipboardNotification, ClipboardWatcher};

const CLIPBOARD_PORT_OFFSET: u16 = 1; // base_port + 1

/// Max wire message size (base64 expands ~33%, plus JSON overhead)
/// Derived from max image size + encoding overhead
fn max_message_size(max_image_bytes: u64) -> usize {
    // base64 expansion (4/3) + JSON framing (~4096 bytes)
    // Use saturating arithmetic to prevent overflow with extreme config values
    let expanded = max_image_bytes.saturating_mul(4) / 3;
    expanded.saturating_add(4096) as usize
}

/// Base64 serde module for efficient binary data in JSON
mod base64_serde {
    use base64::Engine;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(data: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(data))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(&s)
            .map_err(serde::de::Error::custom)
    }
}

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
    /// Text data response (kept as raw bytes for backward compat)
    TextData { data: Vec<u8> },
    /// Image data response (base64-encoded for JSON transport)
    ImageData {
        #[serde(with = "base64_serde")]
        data: Vec<u8>,
    },
}

/// Events from the clipboard sync layer to the service
#[derive(Debug)]
pub enum ClipboardSyncEvent {
    /// Remote clipboard changed (formats available for pull)
    RemoteChanged {
        peer: SocketAddr,
        formats: Vec<ClipboardFormat>,
    },
    /// Received text clipboard data from remote
    RemoteData { text: String },
    /// Received image clipboard data from remote (PNG bytes)
    RemoteImageData { data: Vec<u8> },
}

pub struct ClipboardSync {
    event_rx: mpsc::Receiver<ClipboardSyncEvent>,
}

impl ClipboardSync {
    pub fn new(base_port: u16, max_image_size: u64) -> Self {
        let (event_tx, event_rx) = mpsc::channel(16);
        let clipboard_port = base_port + CLIPBOARD_PORT_OFFSET;

        tokio::task::spawn_local(run_clipboard_server(
            clipboard_port,
            event_tx,
            max_image_size,
        ));

        Self { event_rx }
    }

    pub async fn next_event(&mut self) -> Option<ClipboardSyncEvent> {
        self.event_rx.recv().await
    }
}

async fn run_clipboard_server(
    port: u16,
    event_tx: mpsc::Sender<ClipboardSyncEvent>,
    max_image_size: u64,
) {
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
                        tokio::task::spawn_local(handle_clipboard_peer(stream, addr, etx, max_image_size));
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
    max_image_size: u64,
) {
    let wire_limit = max_message_size(max_image_size);
    #[allow(clippy::while_let_loop)]
    loop {
        // Read length-prefixed JSON messages
        let len = match stream.read_u32().await {
            Ok(l) => l as usize,
            Err(_) => break,
        };
        if len > wire_limit {
            log::warn!("clipboard message too large from {addr}: {len} bytes (limit {wire_limit})");
            break;
        }
        // Dynamically allocate buffer based on message size
        let mut buf = vec![0u8; len];
        if stream.read_exact(&mut buf).await.is_err() {
            break;
        }
        let msg: ClipboardMessage = match serde_json::from_slice(&buf) {
            Ok(m) => m,
            Err(e) => {
                log::warn!("invalid clipboard message from {addr}: {e}");
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
            ClipboardMessage::TextData { data } => {
                if let Ok(text) = String::from_utf8(data) {
                    log::debug!("received clipboard text from {addr}: {} bytes", text.len());
                    let _ = event_tx.send(ClipboardSyncEvent::RemoteData { text }).await;
                }
            }
            ClipboardMessage::ImageData { data } => {
                log::info!("received clipboard image from {addr}: {} bytes", data.len());
                let _ = event_tx
                    .send(ClipboardSyncEvent::RemoteImageData { data })
                    .await;
            }
            ClipboardMessage::Request { format } => {
                log::debug!("clipboard data request from {addr}: {format:?}");
                let provider = crate::clipboard::platform_clipboard();
                match format {
                    ClipboardFormat::Text => {
                        if let Some(text) = provider.get_text() {
                            let response = ClipboardMessage::TextData {
                                data: text.into_bytes(),
                            };
                            if send_message(&mut stream, &response).await.is_err() {
                                break;
                            }
                        }
                    }
                    ClipboardFormat::Image => {
                        if let Some(png_data) = provider.get_image() {
                            if png_data.len() as u64 > max_image_size {
                                log::warn!(
                                    "local image too large to send: {} bytes (limit {})",
                                    png_data.len(),
                                    max_image_size
                                );
                                continue;
                            }
                            let response = ClipboardMessage::ImageData { data: png_data };
                            if send_message(&mut stream, &response).await.is_err() {
                                break;
                            }
                        }
                    }
                    _ => {
                        log::debug!("unsupported clipboard format requested: {format:?}");
                    }
                }
            }
        }
    }
    log::info!("clipboard peer disconnected: {addr}");
}

/// Send a length-prefixed JSON message on a TCP stream
async fn send_message(
    stream: &mut TcpStream,
    msg: &ClipboardMessage,
) -> Result<(), std::io::Error> {
    let json = serde_json::to_vec(msg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    if json.len() > u32::MAX as usize {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "message too large for wire protocol (>4GB)",
        ));
    }
    stream.write_u32(json.len() as u32).await?;
    stream.write_all(&json).await?;
    Ok(())
}

/// Send a clipboard message to a peer (for future active push)
#[allow(dead_code)]
async fn send_clipboard_message(addr: SocketAddr, port: u16, msg: &ClipboardMessage) {
    let clipboard_addr = SocketAddr::new(addr.ip(), port + CLIPBOARD_PORT_OFFSET);
    match TcpStream::connect(clipboard_addr).await {
        Ok(mut stream) => {
            if let Err(e) = send_message(&mut stream, msg).await {
                log::debug!("clipboard send to {clipboard_addr} failed: {e}");
            }
        }
        Err(e) => {
            log::debug!("clipboard connect to {clipboard_addr} failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_data_roundtrip() {
        let msg = ClipboardMessage::TextData {
            data: b"hello world".to_vec(),
        };
        let json = serde_json::to_vec(&msg).unwrap();
        let decoded: ClipboardMessage = serde_json::from_slice(&json).unwrap();
        match decoded {
            ClipboardMessage::TextData { data } => {
                assert_eq!(data, b"hello world");
            }
            _ => panic!("expected TextData"),
        }
    }

    #[test]
    fn image_data_base64_roundtrip() {
        // Minimal PNG header bytes
        let png_bytes: Vec<u8> = vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48,
            0x44, 0x52,
        ];
        let msg = ClipboardMessage::ImageData {
            data: png_bytes.clone(),
        };
        let json = serde_json::to_vec(&msg).unwrap();
        let json_str = String::from_utf8_lossy(&json);
        // Verify base64 encoding is used (not array of numbers)
        assert!(
            json_str.contains("iVBOR"),
            "image data should be base64-encoded"
        );
        assert!(
            !json_str.contains("[137,"),
            "image data should not be JSON array"
        );
        let decoded: ClipboardMessage = serde_json::from_slice(&json).unwrap();
        match decoded {
            ClipboardMessage::ImageData { data } => {
                assert_eq!(data, png_bytes);
            }
            _ => panic!("expected ImageData"),
        }
    }

    #[test]
    fn max_message_size_scales_with_limit() {
        let size_1mb = max_message_size(1024 * 1024);
        let size_50mb = max_message_size(50 * 1024 * 1024);
        assert!(size_50mb > size_1mb);
        // 50MB image -> ~66MB base64 + overhead
        assert!(size_50mb > 60 * 1024 * 1024);
        assert!(size_50mb < 80 * 1024 * 1024);
    }

    #[test]
    fn changed_message_roundtrip() {
        let msg = ClipboardMessage::Changed {
            formats: vec![ClipboardFormat::Text, ClipboardFormat::Image],
            size_hint: 12345,
        };
        let json = serde_json::to_vec(&msg).unwrap();
        let decoded: ClipboardMessage = serde_json::from_slice(&json).unwrap();
        match decoded {
            ClipboardMessage::Changed { formats, size_hint } => {
                assert_eq!(formats.len(), 2);
                assert_eq!(size_hint, 12345);
            }
            _ => panic!("expected Changed"),
        }
    }
}
