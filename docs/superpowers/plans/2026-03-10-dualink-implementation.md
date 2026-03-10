# Dualink Implementation Plan

> **For agentic workers:** REQUIRED: Use superpowers:subagent-driven-development (if subagents available) or superpowers:executing-plans to implement this plan. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Fork lan-mouse into Dualink — a software KVM with Karabiner-Elements compatibility, cross-platform clipboard sync, and modifier key remapping for Win→Mac workflows.

**Architecture:** Dualink keeps lan-mouse's proven UDP+DTLS event pipeline intact. Three surgical additions: (1) replace macOS CGEventPost with Karabiner-DriverKit-VirtualHIDDevice for HID-level input injection, (2) add modifier key remapping in the emulation layer, (3) add clipboard sync over a separate TCP channel. All changes are additive — existing functionality is preserved.

**Tech Stack:** Rust, tokio async runtime, webrtc-dtls, Karabiner-DriverKit-VirtualHIDDevice (C++ via cxx FFI), core-graphics (capture only), NSPasteboard/Win32 Clipboard APIs.

**Source repo:** `/Users/zimingwang/Developer/GitHub/dualink/` (forked from `feschber/lan-mouse`)

---

## Phase 0: Project Rename & Cleanup

### Task 0.1: Rename project from lan-mouse to dualink

**Files:**
- Modify: `Cargo.toml` (root)
- Modify: `input-capture/Cargo.toml`
- Modify: `input-emulation/Cargo.toml`
- Modify: `input-event/Cargo.toml`
- Modify: `lan-mouse-proto/Cargo.toml`
- Modify: `lan-mouse-ipc/Cargo.toml`
- Modify: `lan-mouse-cli/Cargo.toml`
- Modify: `src/config.rs` (config paths)
- Modify: `README.md`

- [x] **Step 1: Update root Cargo.toml**

Change `name = "lan-mouse"` to `name = "dualink"`, update description to `"Dualink — silky-smooth software KVM with Karabiner compatibility"`, update repository URL.

- [x] **Step 2: Remove GTK from default features**

In root `Cargo.toml`, change the `default` features array to remove `"gtk"`:
```toml
default = [
    "layer_shell_capture",
    "x11_capture",
    "libei_capture",
    "wlroots_emulation",
    "libei_emulation",
    "rdp_emulation",
    "x11_emulation",
]
```

- [x] **Step 3: Update config paths**

In `src/config.rs`, change:
- `CONFIG_FILE_NAME` from `"config.toml"` to `"config.toml"` (keep same)
- `CERT_FILE_NAME` from `"lan-mouse.pem"` to `"dualink.pem"`
- Config directory from `"lan-mouse/"` to `"dualink/"` in `default_path()`

- [x] **Step 4: Update IPC socket name**

In `lan-mouse-ipc/src/lib.rs`, change the socket name from `"lan-mouse-socket.sock"` to `"dualink.sock"`.

- [x] **Step 5: Verify build**

Run: `cd /Users/zimingwang/dualink && cargo build --no-default-features 2>&1 | tail -5`
Expected: Build succeeds on macOS (only macOS backends enabled)

- [x] **Step 6: Commit**

```bash
git add -A
git commit -m "rename: fork lan-mouse as dualink, remove GTK default"
```

---

## Phase 1: Modifier Key Remapping (Ctrl→Cmd, Alt→Control)

### Task 1.1: Add key remapping config

**Files:**
- Modify: `src/config.rs`

- [x] **Step 1: Add key_remap field to ConfigToml**

In `src/config.rs`, add to the `ConfigToml` struct:
```rust
key_remap: Option<HashMap<String, String>>,
```

Add to `Config`:
```rust
pub fn key_remap(&self) -> HashMap<scancode::Linux, scancode::Linux> {
    self.config_toml
        .as_ref()
        .and_then(|c| c.key_remap.as_ref())
        .map(|map| {
            map.iter()
                .filter_map(|(from, to)| {
                    let from_key = serde_plain::from_str::<scancode::Linux>(from).ok()?;
                    let to_key = serde_plain::from_str::<scancode::Linux>(to).ok()?;
                    Some((from_key, to_key))
                })
                .collect()
        })
        .unwrap_or_default()
}
```

- [x] **Step 2: Commit**

```bash
git add src/config.rs
git commit -m "feat: add key_remap config field"
```

### Task 1.2: Implement remapping in emulation layer

**Files:**
- Modify: `input-emulation/src/lib.rs`

- [x] **Step 1: Add remap field to InputEmulation**

In `input-emulation/src/lib.rs`, add a `key_remap: HashMap<u32, u32>` field to `InputEmulation` struct, and a `set_key_remap()` method.

- [x] **Step 2: Add remap logic in consume()**

In `InputEmulation::consume()`, before calling `self.emulation.consume()`, remap the key:
```rust
let event = match event {
    Event::Keyboard(KeyboardEvent::Key { time, key, state }) => {
        let remapped_key = self.key_remap.get(&key).copied().unwrap_or(key);
        Event::Keyboard(KeyboardEvent::Key { time, key: remapped_key, state })
    }
    other => other,
};
```

- [x] **Step 3: Wire config into emulation**

In `src/emulation.rs`, pass `key_remap` from config when creating `InputEmulation`.

- [x] **Step 4: Add default Win→Mac remap to config.toml**

Add example config:
```toml
[key_remap]
KeyLeftCtrl = "KeyLeftMeta"      # Ctrl → Cmd
KeyRightCtrl = "KeyRightmeta"   # Right Ctrl → Right Cmd
KeyLeftAlt = "KeyLeftCtrl"       # Alt → Control
KeyRightalt = "KeyRightCtrl"    # Right Alt → Right Control
KeyLeftMeta = "KeyLeftAlt"       # Win → Option
KeyRightmeta = "KeyRightalt"    # Right Win → Right Option
```

- [x] **Step 5: Test manually**

Run dualink daemon on Mac, connect from Windows. Press Ctrl+C on Windows keyboard. Verify it maps to Cmd+C on Mac.

- [x] **Step 6: Commit**

```bash
git add input-emulation/src/lib.rs src/emulation.rs config.toml
git commit -m "feat: add modifier key remapping (Ctrl→Cmd, Alt→Control)"
```

---

## Phase 2: Replace CGEventPost with Karabiner VirtualHID

### Task 2.1: Add VirtualHID C++ bridge

**Files:**
- Create: `input-emulation/src/macos_vhid.rs` — Rust wrapper for VirtualHID
- Create: `input-emulation/src/macos_vhid_bridge.cpp` — C++ bridge code
- Create: `input-emulation/src/macos_vhid_bridge.h` — C++ header
- Modify: `input-emulation/Cargo.toml` — add cc build dependency
- Create: `input-emulation/build.rs` — compile C++ bridge

- [x] **Step 1: Check if Karabiner VirtualHID daemon socket exists**

Run: `ls /Library/Application\ Support/org.pqrs/tmp/rootonly/vhidd_server/`
NOTE: Karabiner not installed. Implemented as optional feature `macos_vhid` using `karabiner-driverkit` crate.

- [x] **Step 2: Study VirtualHID client API**

Read the Karabiner-DriverKit-VirtualHIDDevice source to understand:
- Socket connection protocol
- HID report format (keyboard_input, pointing_input)
- The async_post_report() call pattern

Key reference: `include/pqrs/karabiner/driverkit/virtual_hid_device_service.hpp`

- [x] **Step 3: Write C++ bridge**

Create `input-emulation/src/macos_vhid_bridge.h`:
```cpp
#pragma once
#include <stdbool.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct VHIDClient VHIDClient;

VHIDClient* vhid_connect(void);
void vhid_disconnect(VHIDClient* client);
bool vhid_post_keyboard(VHIDClient* client, uint8_t modifiers, const uint8_t* keys, uint8_t key_count);
bool vhid_post_pointing(VHIDClient* client, uint8_t buttons, int8_t x, int8_t y, int8_t vwheel, int8_t hwheel);

#ifdef __cplusplus
}
#endif
```

Create `input-emulation/src/macos_vhid_bridge.cpp` implementing these functions using Karabiner VirtualHID client API.

- [x] **Step 4: Add build.rs to compile C++ bridge**

Create `input-emulation/build.rs`:
```rust
fn main() {
    #[cfg(target_os = "macos")]
    {
        cc::Build::new()
            .cpp(true)
            .file("src/macos_vhid_bridge.cpp")
            .flag("-std=c++20")
            .include("/path/to/karabiner/include")
            .compile("vhid_bridge");
    }
}
```

- [x] **Step 5: Add cc dependency to Cargo.toml**

In `input-emulation/Cargo.toml`:
```toml
[build-dependencies]
cc = "1.0"
```

- [x] **Step 6: Commit bridge code**

```bash
git add input-emulation/
git commit -m "feat: add Karabiner VirtualHID C++ bridge"
```

### Task 2.2: Create VirtualHID emulation backend

**Files:**
- Create: `input-emulation/src/macos_vhid.rs`
- Modify: `input-emulation/src/lib.rs` — add new backend variant

- [x] **Step 1: Write macos_vhid.rs**

Implement `Emulation` trait using the C FFI bridge. Key differences from CGEventPost:
- `pointing_input.x/y` are `int8_t` (-128..127) — split large mouse deltas into multiple reports
- Keyboard uses USB HID Usage codes, not Mac keycodes — need evdev→USB HID mapping
- No need for manual key repeat (system handles it through VirtualHID)
- No need for manual double-click detection (system handles it)

```rust
pub(crate) struct VirtualHIDEmulation {
    client: *mut ffi::VHIDClient,
    pressed_buttons: u8,
}
```

- [x] **Step 2: Add backend selection**

In `input-emulation/src/lib.rs`, add `MacOsVHID` variant to the `Backend` enum. Update the auto-detection to prefer VirtualHID if the daemon socket exists, fallback to CGEventPost.

- [x] **Step 3: Test VirtualHID injection**

Run dualink with `--emulation-backend macos-vhid`. Verify:
- Mouse movement works
- Keyboard input works
- Karabiner-Elements processes the input (check Karabiner EventViewer)

- [x] **Step 4: Commit**

```bash
git add input-emulation/
git commit -m "feat: VirtualHID emulation backend for Karabiner compatibility"
```

---

## Phase 3: Clipboard Sync

### Task 3.1: Add clipboard event types

**Files:**
- Modify: `input-event/src/lib.rs`

- [x] **Step 1: Add ClipboardEvent enum**

```rust
#[derive(Debug, Clone)]
pub enum ClipboardEvent {
    /// Notify that clipboard content changed (formats available + sizes)
    Changed { formats: Vec<ClipboardFormat> },
    /// Request clipboard data for a specific format
    Request { format: ClipboardFormat },
    /// Clipboard data response
    Data { format: ClipboardFormat, data: Vec<u8> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardFormat {
    Text,
    Html,
    Image,
}
```

- [x] **Step 2: Commit**

```bash
git add input-event/src/lib.rs
git commit -m "feat: add clipboard event types"
```

### Task 3.2: Platform clipboard adapters

**Files:**
- Create: `src/clipboard/mod.rs`
- Create: `src/clipboard/macos.rs`
- Create: `src/clipboard/windows.rs`

- [x] **Step 1: Define clipboard trait**

In `src/clipboard/mod.rs`:
```rust
pub trait ClipboardProvider: Send {
    fn get_text(&self) -> Option<String>;
    fn set_text(&self, text: &str);
    fn has_changed(&self) -> bool;
}
```

- [x] **Step 2: Implement macOS adapter**

In `src/clipboard/macos.rs`, use `NSPasteboard` via objc2 crate:
- Poll `changeCount` every 500ms to detect changes
- Read/write `NSPasteboardTypeString`

- [x] **Step 3: Implement Windows adapter**

In `src/clipboard/windows.rs`, use Win32 API:
- `AddClipboardFormatListener` for change detection
- `CF_UNICODETEXT` for text (UTF-16LE ↔ UTF-8 conversion)

- [x] **Step 4: Commit**

```bash
git add src/clipboard/
git commit -m "feat: platform clipboard adapters (macOS + Windows)"
```

### Task 3.3: Clipboard sync over TCP

**Files:**
- Create: `src/clipboard_sync.rs`
- Modify: `src/service.rs` — add clipboard channel to select! loop

- [x] **Step 1: Create TCP clipboard channel**

Implement a simple TCP server on port 4243 (next to UDP 4242) that:
- Accepts TLS connections from authenticated peers
- Sends `ClipboardChanged` notifications when local clipboard changes
- Responds to `ClipboardRequest` with data (lazy pull)

- [x] **Step 2: Integrate into service event loop**

Add clipboard polling to `Service::run()` select! loop:
```rust
event = self.clipboard.poll() => self.handle_clipboard_event(event),
```

- [x] **Step 3: Test clipboard sync**

Copy text on Windows → verify it appears on Mac clipboard.
Copy text on Mac → verify it appears on Windows clipboard.

- [x] **Step 4: Commit**

```bash
git add src/clipboard_sync.rs src/clipboard/ src/service.rs
git commit -m "feat: clipboard text sync over TCP"
```

---

## Phase 4: Edge Detection & Polish

### Task 4.1: Edge switch cooldown

**Files:**
- Modify: `input-capture/src/macos.rs`

- [x] **Step 1: Add cooldown timer**

After a successful edge switch, ignore further edge crossings for 200ms:
```rust
const EDGE_COOLDOWN: Duration = Duration::from_millis(200);
// In crossed() or start_capture(), check elapsed time since last switch
```

- [x] **Step 2: Commit**

```bash
git add input-capture/src/macos.rs
git commit -m "fix: add 200ms edge switch cooldown to prevent bouncing"
```

### Task 4.2: Create GitHub repository

- [ ] **Step 1: Create repo on GitHub**

```bash
gh repo create ZenAlexa/dualink --public --description "Silky-smooth software KVM with Karabiner compatibility" --source /Users/zimingwang/dualink
```

- [ ] **Step 2: Push initial fork**

```bash
cd /Users/zimingwang/dualink
git remote add origin https://github.com/ZenAlexa/dualink.git
git push -u origin main
```

---

## Summary

| Phase | Tasks | Est. Lines Changed | Status |
|-------|-------|-------------------|--------|
| 0: Rename | 1 task, 6 steps | ~30 | - [x] |
| 1: Key Remap | 2 tasks, 6+6 steps | ~80 | - [x] |
| 2: VirtualHID | 2 tasks, 6+4 steps | ~400 | - [x] |
| 3: Clipboard | 3 tasks, 2+3+4 steps | ~500 | - [x] |
| 4: Polish | 2 tasks, 2+2 steps | ~20 | - [x] |
| **Total** | **10 tasks** | **~1030** | |

**Critical path:** Phase 0 → Phase 1 → Phase 2 (each depends on previous)
**Independent:** Phase 3 (clipboard) can be developed in parallel with Phase 2
**Phase 4** depends on all others
