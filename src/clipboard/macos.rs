use super::ClipboardProvider;
use std::process::Command;

/// macOS clipboard provider using pbcopy/pbpaste
/// Uses NSPasteboard changeCount via osascript for change detection
pub struct MacOSClipboard {
    _private: (),
}

impl MacOSClipboard {
    pub fn new() -> Self {
        Self { _private: () }
    }

    fn get_change_count_inner() -> u64 {
        let output = Command::new("osascript")
            .args([
                "-e",
                "tell application \"System Events\" to return (the clipboard info)",
            ])
            .output();
        // Use a simpler approach: hash the clipboard content type info
        match output {
            Ok(out) => {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                out.stdout.hash(&mut hasher);
                hasher.finish()
            }
            Err(_) => 0,
        }
    }
}

unsafe impl Send for MacOSClipboard {}

impl ClipboardProvider for MacOSClipboard {
    fn get_text(&self) -> Option<String> {
        let output = Command::new("pbpaste").output().ok()?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        } else {
            None
        }
    }

    fn set_text(&self, text: &str) {
        use std::io::Write;
        let mut child = match Command::new("pbcopy").stdin(std::process::Stdio::piped()).spawn() {
            Ok(c) => c,
            Err(e) => {
                log::warn!("failed to spawn pbcopy: {e}");
                return;
            }
        };
        if let Some(ref mut stdin) = child.stdin {
            let _ = stdin.write_all(text.as_bytes());
        }
        let _ = child.wait();
    }

    fn get_change_count(&self) -> u64 {
        Self::get_change_count_inner()
    }
}
