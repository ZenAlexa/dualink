use input_event::ClipboardFormat;
use std::time::Duration;
use tokio::sync::mpsc;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(windows)]
mod windows;

/// Notification from the clipboard watcher
#[derive(Debug, Clone)]
pub enum ClipboardNotification {
    /// Local clipboard content changed
    Changed {
        formats: Vec<ClipboardFormat>,
        size_hint: u64,
    },
}

/// Platform clipboard provider trait
pub trait ClipboardProvider: Send + 'static {
    fn get_text(&self) -> Option<String>;
    fn set_text(&self, text: &str);
    fn get_change_count(&self) -> u64;

    /// Get image data from clipboard as PNG bytes
    fn get_image(&self) -> Option<Vec<u8>> {
        None
    }
    /// Set clipboard image from PNG bytes
    fn set_image(&self, _data: &[u8]) {}
    /// Check if clipboard contains an image (without reading it)
    fn has_image(&self) -> bool {
        false
    }
}

/// Clipboard watcher that polls for changes
pub struct ClipboardWatcher {
    rx: mpsc::Receiver<ClipboardNotification>,
}

impl ClipboardWatcher {
    pub fn new(poll_interval: Duration) -> Self {
        let (tx, rx) = mpsc::channel(8);

        #[cfg(target_os = "macos")]
        {
            let provider = macos::MacOSClipboard::new();
            tokio::task::spawn_local(poll_clipboard(provider, tx, poll_interval));
        }

        #[cfg(windows)]
        {
            let provider = windows::WindowsClipboard::new();
            tokio::task::spawn_local(poll_clipboard(provider, tx, poll_interval));
        }

        #[cfg(not(any(target_os = "macos", windows)))]
        {
            let _ = (tx, poll_interval);
            log::warn!("clipboard sync not supported on this platform");
        }

        Self { rx }
    }

    pub async fn next(&mut self) -> Option<ClipboardNotification> {
        self.rx.recv().await
    }
}

async fn poll_clipboard<P: ClipboardProvider>(
    provider: P,
    tx: mpsc::Sender<ClipboardNotification>,
    interval: Duration,
) {
    let mut last_count = provider.get_change_count();
    loop {
        tokio::time::sleep(interval).await;
        let count = provider.get_change_count();
        if count != last_count {
            last_count = count;
            let mut formats = Vec::new();
            let mut size_hint = 0u64;
            if let Some(text) = provider.get_text() {
                size_hint = text.len() as u64;
                formats.push(ClipboardFormat::Text);
            }
            if provider.has_image() {
                formats.push(ClipboardFormat::Image);
            }
            if !formats.is_empty() {
                let _ = tx
                    .send(ClipboardNotification::Changed { formats, size_hint })
                    .await;
            }
        }
    }
}

/// Get the platform clipboard provider (for reading/writing from sync layer)
#[cfg(target_os = "macos")]
pub fn platform_clipboard() -> Box<dyn ClipboardProvider> {
    Box::new(macos::MacOSClipboard::new())
}

#[cfg(windows)]
pub fn platform_clipboard() -> Box<dyn ClipboardProvider> {
    Box::new(windows::WindowsClipboard::new())
}

#[cfg(not(any(target_os = "macos", windows)))]
pub fn platform_clipboard() -> Box<dyn ClipboardProvider> {
    Box::new(DummyClipboard)
}

#[cfg(not(any(target_os = "macos", windows)))]
struct DummyClipboard;

#[cfg(not(any(target_os = "macos", windows)))]
impl ClipboardProvider for DummyClipboard {
    fn get_text(&self) -> Option<String> {
        None
    }
    fn set_text(&self, _text: &str) {}
    fn get_change_count(&self) -> u64 {
        0
    }
}
