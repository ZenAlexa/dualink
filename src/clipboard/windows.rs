use super::ClipboardProvider;

/// Windows clipboard provider using Win32 clipboard APIs
pub struct WindowsClipboard {
    last_sequence: u32,
}

impl WindowsClipboard {
    pub fn new() -> Self {
        Self {
            last_sequence: Self::get_sequence_number(),
        }
    }

    fn get_sequence_number() -> u32 {
        #[cfg(windows)]
        {
            use windows::Win32::System::DataExchange::GetClipboardSequenceNumber;
            unsafe { GetClipboardSequenceNumber() }
        }
        #[cfg(not(windows))]
        0
    }
}

unsafe impl Send for WindowsClipboard {}

impl ClipboardProvider for WindowsClipboard {
    fn get_text(&self) -> Option<String> {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::System::DataExchange::{
                CloseClipboard, GetClipboardData, OpenClipboard,
            };
            use windows::Win32::System::Memory::GlobalLock;
            use windows::Win32::System::Memory::GlobalUnlock;
            use windows::Win32::System::Ole::CF_UNICODETEXT;

            unsafe {
                if !OpenClipboard(HWND::default()).is_ok() {
                    return None;
                }

                let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok();
                let result = handle.and_then(|h| {
                    let ptr = GlobalLock(std::mem::transmute(h.0)) as *const u16;
                    if ptr.is_null() {
                        return None;
                    }
                    let mut len = 0;
                    while *ptr.add(len) != 0 {
                        len += 1;
                    }
                    let slice = std::slice::from_raw_parts(ptr, len);
                    let text = String::from_utf16_lossy(slice);
                    GlobalUnlock(std::mem::transmute(h.0));
                    Some(text)
                });

                let _ = CloseClipboard();
                result
            }
        }
        #[cfg(not(windows))]
        None
    }

    fn set_text(&self, _text: &str) {
        #[cfg(windows)]
        {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::System::DataExchange::{
                CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
            };
            use windows::Win32::System::Memory::{
                GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
            };
            use windows::Win32::System::Ole::CF_UNICODETEXT;

            let wide: Vec<u16> = _text.encode_utf16().chain(std::iter::once(0)).collect();
            let size = wide.len() * 2;

            unsafe {
                if !OpenClipboard(HWND::default()).is_ok() {
                    return;
                }
                let _ = EmptyClipboard();

                if let Ok(hmem) = GlobalAlloc(GMEM_MOVEABLE, size) {
                    let ptr = GlobalLock(hmem) as *mut u16;
                    if !ptr.is_null() {
                        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                        GlobalUnlock(hmem);
                        let _ = SetClipboardData(CF_UNICODETEXT.0 as u32, std::mem::transmute(hmem));
                    }
                }
                let _ = CloseClipboard();
            }
        }
    }

    fn get_change_count(&self) -> u64 {
        Self::get_sequence_number() as u64
    }
}
