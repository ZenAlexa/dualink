use super::ClipboardProvider;
use std::process::Command;

/// macOS clipboard provider using pbcopy/pbpaste for text,
/// osascript + NSPasteboard for image data (PNG interchange format).
/// Temp files use PID-qualified paths to avoid concurrent access issues.
pub struct MacOSClipboard {
    _private: (),
}

impl MacOSClipboard {
    pub fn new() -> Self {
        Self { _private: () }
    }

    /// Query clipboard info via osascript (returns raw output string)
    fn get_clipboard_info() -> Option<String> {
        let output = Command::new("osascript")
            .args([
                "-e",
                "tell application \"System Events\" to return (the clipboard info)",
            ])
            .output()
            .ok()?;
        Some(String::from_utf8_lossy(&output.stdout).into_owned())
    }

    fn get_change_count_inner() -> u64 {
        match Self::get_clipboard_info() {
            Some(info) => {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                info.hash(&mut hasher);
                hasher.finish()
            }
            None => 0,
        }
    }

    /// Check if clipboard info contains image types
    fn info_has_image(info: &str) -> bool {
        info.contains("PNGf")
            || info.contains("TIFF")
            || info.contains("«class PNGf»")
            || info.contains("«class TIFF»")
    }

    fn temp_path(suffix: &str) -> String {
        format!("/tmp/dualink_cb_{}_{suffix}", std::process::id())
    }
}

unsafe impl Send for MacOSClipboard {}

impl ClipboardProvider for MacOSClipboard {
    fn get_text(&self) -> Option<String> {
        let output = Command::new("pbpaste").output().ok()?;
        if output.status.success() {
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            if text.is_empty() { None } else { Some(text) }
        } else {
            None
        }
    }

    fn set_text(&self, text: &str) {
        use std::io::Write;
        let mut child = match Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
        {
            Ok(c) => c,
            Err(e) => {
                log::warn!("failed to spawn pbcopy: {e}");
                return;
            }
        };
        if let Some(ref mut stdin) = child.stdin {
            if let Err(e) = stdin.write_all(text.as_bytes()) {
                log::warn!("failed to write to pbcopy stdin: {e}");
            }
        }
        let _ = child.wait();
    }

    fn get_change_count(&self) -> u64 {
        Self::get_change_count_inner()
    }

    fn get_image(&self) -> Option<Vec<u8>> {
        let png_path = Self::temp_path("read.png");
        let tiff_path = Self::temp_path("read.tiff");
        // Try to read clipboard as PNG, falling back to TIFF with sips conversion
        let script = format!(
            concat!(
                "try\n",
                "  set pngData to (the clipboard as «class PNGf»)\n",
                "  set outFile to open for access POSIX file \"{}\" with write permission\n",
                "  set eof of outFile to 0\n",
                "  write pngData to outFile\n",
                "  close access outFile\n",
                "  return \"ok:png\"\n",
                "on error\n",
                "  try\n",
                "    set tiffData to (the clipboard as «class TIFF»)\n",
                "    set outFile to open for access POSIX file \"{}\" with write permission\n",
                "    set eof of outFile to 0\n",
                "    write tiffData to outFile\n",
                "    close access outFile\n",
                "    return \"ok:tiff\"\n",
                "  on error\n",
                "    return \"no_image\"\n",
                "  end try\n",
                "end try",
            ),
            png_path, tiff_path,
        );
        let output = Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .ok()?;
        let result = String::from_utf8_lossy(&output.stdout).trim().to_string();

        let data = if result == "ok:png" {
            let data = std::fs::read(&png_path).ok();
            let _ = std::fs::remove_file(&png_path);
            data
        } else if result == "ok:tiff" {
            // Convert TIFF to PNG using macOS sips utility
            let convert_ok = Command::new("sips")
                .args(["-s", "format", "png", &tiff_path, "--out", &png_path])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false);
            let _ = std::fs::remove_file(&tiff_path);
            if convert_ok {
                let data = std::fs::read(&png_path).ok();
                let _ = std::fs::remove_file(&png_path);
                data
            } else {
                let _ = std::fs::remove_file(&png_path);
                None
            }
        } else {
            None
        };

        // Validate PNG magic bytes
        match data {
            Some(d) if d.len() >= 8 && d[..4] == [0x89, 0x50, 0x4E, 0x47] => Some(d),
            Some(_) => {
                log::warn!("clipboard image data does not have valid PNG header");
                None
            }
            None => None,
        }
    }

    fn set_image(&self, data: &[u8]) {
        let temp_path = Self::temp_path("write.png");
        if let Err(e) = std::fs::write(&temp_path, data) {
            log::warn!("failed to write temp image file: {e}");
            return;
        }
        // Use osascript to set clipboard from PNG file
        let script = format!(
            concat!(
                "try\n",
                "  set imgFile to POSIX file \"{}\"\n",
                "  set imgData to read imgFile as «class PNGf»\n",
                "  set the clipboard to {{«class PNGf»:imgData}}\n",
                "  return \"ok\"\n",
                "on error errMsg\n",
                "  return \"error:\" & errMsg\n",
                "end try",
            ),
            temp_path,
        );
        let output = Command::new("osascript").arg("-e").arg(&script).output();
        match output {
            Ok(out) => {
                let result = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if result.starts_with("error:") {
                    log::warn!("failed to set clipboard image: {result}");
                } else {
                    log::debug!("clipboard image set: {} bytes", data.len());
                }
            }
            Err(e) => log::warn!("failed to run osascript for clipboard image: {e}"),
        }
        let _ = std::fs::remove_file(&temp_path);
    }

    fn has_image(&self) -> bool {
        Self::get_clipboard_info()
            .map(|info| Self::info_has_image(&info))
            .unwrap_or(false)
    }
}
