# Progress Log

## Cross-Reference Ledger

| Phase | Status | Key Outcome |
|-------|--------|-------------|
| Competitive Analysis | COMPLETED | findings.md written |
| Phase 1: Tahoe Resilience | DONE | CGEventTap hardening, accessibility check, diagnostics |
| Phase 2: Mouse Quality | DONE | Event coalescing, mouse/scroll speed, natural scrolling, batch protocol, diagnostics |
| Phase 3: Key Remapping | DONE + REVIEWED | Session 3.1: modifier-aware remapping; Session 3.2: hot-reload + IPC; Session 3.3: special key handling; cross-review: 3 fixes applied |
| Phase 4: Clipboard | NOT STARTED | — |
| Phase 5: Polish | NOT STARTED | — |

---

## Session: 2026-03-11 — Competitive Analysis

### What Happened
1. Cloned 4 reference repos: deskflow, input-leap, barrier, upstream lan-mouse
2. Ran 5 parallel analysis agents:
   - dualink deep codebase analysis (8 areas)
   - deskflow clipboard/mouse/keys/protocol/macOS
   - input-leap + barrier comparison
   - dualink vs upstream lan-mouse diff
   - web research: latency benchmarks, macOS Tahoe changes, VirtualHID, clipboard, DPI
3. Synthesized findings into gap analysis and prioritized task plan

### Key Discoveries
- **dualink already has the right transport** (UDP+DTLS) — competitors all use TCP
- **Mouse quality is the #1 gap** — no coalescing, no acceleration handling, no DPI normalization
- **macOS Tahoe has breaking changes** — CGEventTap silent disable, CGEventPost partial failure, permission loops
- **Clipboard is text-only** — competitors support HTML + images, input-leap supports file drag
- **Key remapping is too basic** — competitors have 3-layer systems with keyboard groups
- **VirtualHID value confirmed** — critical for Tahoe where CGEventPost is degraded
- **High poll rate mice (>1000Hz)** are a universal pain point for software KVMs

### Errors
None.

### Next Session
Begin Phase 1: macOS Tahoe resilience (Session 1.1: CGEventTap hardening)

---

## Session: 2026-03-11 — Phase 1: Tahoe Resilience

### What Happened

#### Session 1.1: CGEventTap Hardening
1. **Accessibility check at startup**: Added `AXIsProcessTrusted()` call in `create_event_tap()` before creating the tap. Returns clear `AccessibilityNotTrusted` error with actionable message.
2. **Auto-re-enable on timeout**: Stored mach port in `Arc<AtomicPtr>` shared with callback. On `TapDisabledByTimeout`, callback calls `CGEventTapEnable(port, true)` to re-enable — matches deskflow's approach (OSXScreen.mm:1661).
3. **Clean shutdown on user-input disable**: On `TapDisabledByUserInput` (accessibility revoked), sends `EventTapDisabled` notification AND stops the CFRunLoop to trigger proper capture teardown.
4. **Separated timeout vs user-input handling**: Previously both were treated identically (fatal error). Now timeout is auto-recovered (transient), user-input is fatal (permission revoked).

#### Session 1.2: VirtualHID Backend
- Backend priority already correct (VirtualHID listed before CGEventPost in `input-emulation/src/lib.rs:138-139`)
- VirtualHID already checks socket existence + driver activation at construction time
- Fallback to CGEventPost already logs at warn level
- No additional changes needed — existing implementation matches the plan

#### Session 1.3: Permission & Diagnostics
1. **Created `src/diagnostics.rs`** with two entry points:
   - `log_startup_checks()`: Runs at service start, logs warnings for issues
   - `print_full_report()`: Full `--diagnose` output to stdout
2. **Checks implemented**:
   - macOS version detection via `sw_vers`
   - Accessibility permission via `AXIsProcessTrusted()`
   - Secure Input mode via `IsSecureEventInputEnabled()` (Carbon)
   - VirtualHID daemon socket + driver extension presence
   - Network port availability (UDP bind test)
   - macOS Tahoe detection with specific warning
3. **`--diagnose` CLI flag** added to Args and Config
4. **Startup diagnostics** called in `run_service()` before creating the service

### Plan Divergences
- **Session 1.2 was already implemented**: VirtualHID auto-detect, fallback, and health check were already in place from a previous session. No code changes needed.
- **Periodic TCC monitoring deferred**: The plan called for periodic accessibility permission polling. Instead, we rely on: (a) startup check, (b) `TapDisabledByUserInput` runtime detection. This is simpler and matches deskflow's approach. Can add periodic polling later if needed.
- **Secure Input PID detection simplified**: Plan referenced deskflow's IORegistry-based PID lookup. Used `IsSecureEventInputEnabled()` boolean check instead — simpler, and the full diagnostic report provides actionable guidance without knowing the exact app.

### Verification
- `cargo build --no-default-features` ✓
- `cargo build --no-default-features --features macos_vhid` ✓
- `cargo fmt --check` ✓
- `cargo clippy --no-default-features -- -D warnings` ✓
- `cargo test --no-default-features` ✓
- `cargo run --no-default-features -- --diagnose` ✓ (full report verified)

### Cross-Verification Fixes (Codex MCP + GemSuite MCP)
1. **[FIXED] blocking_lock() before tap disable checks** — Moved TapDisabledByTimeout/UserInput handling BEFORE `client_state.blocking_lock()`. The timeout handler only needs the AtomicPtr, not the mutex. This prevents mutex contention from causing cascading timeouts.
2. **[FIXED] AccessibilityNotTrusted error lost in auto-selection** — Added `is_permission_error()` method to `CaptureCreationError`. Backend auto-selection now short-circuits on permission errors instead of falling through to `NoAvailableBackend`.
3. **[FIXED] --diagnose hardcoded DEFAULT_PORT** — `print_full_report()` now takes a `port` parameter from config. Verified: `--port 5000 --diagnose` correctly shows "UDP port 5000".
4. **[FIXED] Null mach port leaves tap permanently disabled** — Changed fallback from silent log to sending `EventTapDisabled` + stopping CFRunLoop, triggering a full capture restart.

### Errors
None.

### Files Changed
| File | Change |
|------|--------|
| `input-capture/src/error.rs` | Added `AccessibilityNotTrusted` error variant |
| `input-capture/src/macos.rs` | CGEventTap hardening: accessibility check, auto-re-enable, clean shutdown |
| `src/diagnostics.rs` | NEW: startup diagnostics + `--diagnose` report |
| `src/config.rs` | Added `--diagnose` CLI flag |
| `src/lib.rs` | Added `pub mod diagnostics` |
| `src/main.rs` | Handle `--diagnose`, startup checks, cfg-gate GTK imports |
| `src/clipboard_sync.rs` | Suppressed pre-existing clippy warning |
| Various | `cargo fmt` formatting fixes |

### Next Session
Begin Phase 2: Mouse Quality & Latency (Session 2.1: Event coalescing)

---

## Session: 2026-03-11 — Phase 2: Mouse Quality & Latency

### What Happened

#### Session 2.1: Event Coalescing for High-Poll-Rate Mice
1. **Created `src/event_coalescer.rs`** — standalone module with 6 unit tests
2. **Accumulates mouse motion deltas** within configurable time window (default 1ms)
3. **Preserves all non-motion events** (buttons, keys, scroll) — flushes accumulated motion first to maintain ordering
4. **Configurable via `coalesce_window_us`** in config.toml (0 = disabled)
5. **Integrated into CaptureTask** select! loop with timer-based flush arm
6. **Disabled coalescer (window=0)** is a true no-op passthrough

#### Session 2.2: Mouse Speed & DPI Awareness
1. **Added `mouse_speed` config** (f64, default 1.0) — multiplier applied to all incoming pointer motion events
2. **Applied at `InputEmulation::consume()` level** — transforms events before dispatch to platform backend, like key_remap
3. **Reads `com.apple.mouse.scaling` system preference** — displayed in `--diagnose` report for user awareness
4. **MouseConfig struct** flows through: Config → Service → Emulation → EmulationProxy → EmulationTask → InputEmulation

#### Session 2.3: Network Event Batching
1. **Added batch encoding/decoding** in `lan-mouse-proto/src/lib.rs`
2. **Wire format**: `[0xFF magic][count:u8]([len:u8][event_data...])*`
3. **Backward compatible**: receiver detects batch by 0xFF first byte (no valid EventType uses 0xFF)
4. **Size-safe**: encode_batch enforces MAX_BATCH_SIZE (1200 bytes, within MTU)
5. **Receiver upgraded** to MAX_BATCH_SIZE buffer, uses `decode_packet()` for transparent single/batch handling
6. **`send_batch()` API added** to LanMouseConnection for future batch sending

#### Session 2.4: Scroll Wheel Normalization
1. **Added `scroll_speed` config** (f64, default 1.0) — multiplier for scroll events
2. **Natural scrolling support** — `natural_scrolling` config (Option<bool>, None = follow system preference)
3. **System preference detection** — reads `com.apple.swipescrolldirection` at startup, falls back to macOS default (true)
4. **Both discrete (line) and continuous (pixel) scroll modes** handled
5. **AxisDiscrete120 precision** — uses round-to-nearest with minimum 1-unit floor to prevent zero-collapse at sub-1.0 speeds
6. **Reads `com.apple.scrollwheel.scaling`** — displayed in diagnostics

### Cross-Verification Fixes (Codex MCP)
1. **[FIXED] P1: Buffered motion sent to wrong client** — Flush coalescer BEFORE updating `active_client` when switching clients on Begin event. Previously, accumulated motion for client A would leak to client B.
2. **[FIXED] P1: Batch size not enforced** — `encode_batch()` now stops adding events when MAX_BATCH_SIZE would be exceeded, instead of silently exceeding MTU.
3. **[FIXED] P1: Truncated batch partial decode** — `decode_packet()` now logs warnings on truncated batches instead of silently returning partial results.
4. **[FIXED] P1: Timer flush ignoring send failures** — `flush_coalesced_motion()` now returns error status; timer arm releases capture on failure instead of leaving it in limbo.
5. **[FIXED] P2: AxisDiscrete120 scroll precision** — Changed from truncation to round-to-nearest with minimum 1-unit floor, preventing zero-collapse at sub-1.0 scroll speeds.

### Plan Divergences
- **DPI normalization deferred**: Adding screen resolution to the Enter protocol message requires protocol versioning and both-side changes. The `mouse_speed` config multiplier provides equivalent user-level control. Can add protocol-level DPI negotiation in a future phase.
- **Network batch sending not yet active**: The batch encode/decode and `send_batch()` API are implemented, but the capture path currently uses the coalescer (which reduces events) + individual sends. The batch infrastructure is ready for integration when needed.
- **Benchmark not implemented**: Measuring event rates requires a two-machine test setup. The coalescer is ready for benchmarking once the Windows side is available.

### Verification
- `cargo build --no-default-features` ✓
- `cargo build --no-default-features --features macos_vhid` ✓
- `cargo fmt --check` ✓
- `cargo clippy --no-default-features -- -D warnings` ✓
- `cargo test --no-default-features` ✓ (6 coalescer tests + 2 proto batch tests)
- `cargo test -p lan-mouse-proto` ✓ (batch_roundtrip + legacy_single_event_decode)
- `cargo run --no-default-features -- --diagnose` ✓ (mouse/scroll prefs displayed)
- Codex MCP cross-verification: 4 P1 bugs found and fixed, 1 P2 fixed

### Errors
None.

### Files Changed
| File | Change |
|------|--------|
| `src/event_coalescer.rs` | NEW: Motion event coalescing with configurable window and 6 unit tests |
| `src/lib.rs` | Added `pub mod event_coalescer` |
| `src/config.rs` | Added mouse_speed, scroll_speed, natural_scrolling, coalesce_window_us config fields |
| `src/capture.rs` | Integrated EventCoalescer: timer flush arm, coalesced sending, flush-before-switch |
| `src/connect.rs` | Batch receive (MAX_BATCH_SIZE buffer + decode_packet), send_batch API |
| `src/service.rs` | Pass MouseConfig and coalesce_window to Emulation and Capture; natural scrolling detection |
| `src/emulation.rs` | Thread MouseConfig through EmulationProxy → EmulationTask → InputEmulation |
| `src/diagnostics.rs` | Added mouse scaling, scroll scaling, natural scrolling to diagnostic report |
| `input-emulation/src/lib.rs` | MouseConfig struct, apply_mouse_config (speed + scroll + natural scrolling), set_mouse_config |
| `lan-mouse-proto/src/lib.rs` | Batch encode/decode (0xFF magic, size-safe), 2 unit tests |
| `lan-mouse-proto/Cargo.toml` | Added `log` dependency |

### Next Session
Begin Phase 3: Key Remapping Overhaul (Session 3.1: Modifier-aware remapping engine)

---

## Session: 2026-03-15 — Phase 3: Session 3.1

### What Happened

#### Session 3.1: Modifier-Aware Remapping Engine
1. **Created `src/keymap.rs`** — standalone module with `ModifierRole` enum, `KeyRemapConfig`, and `KeyRemapEngine` (9 unit tests)
2. **Modifier role remapping**: `ModifierRole` maps friendly config names ("ctrl", "cmd", "option", "win") to left/right scancode pairs and XMods bitmasks
3. **Modifier state tracking**: `pressed_modifiers: HashMap<u32, u32>` records physical→remapped mapping on press, ensures correct release (stickiness)
4. **Modifier bitmask remapping**: `remap_modifier_bits()` transforms `Modifiers` event `depressed` field — reads original bits, clears source bits, sets destination bits atomically (handles circular swaps correctly)
5. **New TOML config format**: `[key_remap.modifiers]` for role remapping (e.g., `ctrl = "cmd"`), `[key_remap.keys]` for scancode remapping (e.g., `"KeyCapsLock" = "KeyEsc"`)
6. **Architectural refactor**: moved key remapping from `input-emulation` library crate to service layer (`EmulationTask` in `src/emulation.rs`), keeping the library backend-agnostic
7. **Removed old code**: `key_remap: HashMap<u32, u32>` field, `set_key_remap()` method, and inline remap logic removed from `InputEmulation::consume()`

### Plan Divergences
- **Per-key modifier override deferred**: The task plan mentions "Ctrl+C → Cmd+C but Ctrl+Alt+Del unchanged" exclusion rules. The modifier remapping inherently handles Ctrl+C → Cmd+C (since Ctrl is globally remapped to Cmd). Multi-modifier exclusions would require a more complex rule engine — deferred to Session 3.2 or later if needed.
- **Config backward compatibility**: The old flat `[key_remap]` format (`"KeyLeftCtrl" = "KeyLeftMeta"`) is silently ignored under the new structured format. This is a breaking change, acceptable for early-stage project.

### Cross-Verification
- **Code reviewer (sonnet)**: 0 critical, 2 warnings (global reset for single-client assumption — documented with comment; undocumented pressed_keys contract — added comment), 4 suggestions (comments added)
- **GemSuite (gemini_reason)**: Verified bitmask algorithm across 8 scenarios including circular swaps and chain remaps. Verified stickiness algorithm for config-change and orphaned-release cases. Both algorithms confirmed correct.

### Verification
- `cargo build --no-default-features` ✓
- `cargo build --no-default-features --features macos_vhid` ✓
- `cargo fmt --check` ✓
- `cargo clippy --no-default-features -- -D warnings` ✓
- `cargo test --no-default-features` ✓ (15 tests: 9 keymap + 6 coalescer)

### Errors
None.

### Files Changed
| File | Change |
|------|--------|
| `src/keymap.rs` | NEW: ModifierRole, KeyRemapConfig, KeyRemapEngine with 9 unit tests |
| `src/lib.rs` | Added `pub mod keymap` |
| `src/config.rs` | KeyRemapToml struct, changed key_remap field type, new key_remap_config() accessor |
| `src/service.rs` | Use new config.key_remap_config() accessor |
| `src/emulation.rs` | KeyRemapConfig/Engine types, remap in do_emulation_session(), reset on Remove |
| `input-emulation/src/lib.rs` | Removed key_remap field, set_key_remap(), and remap logic from consume() |

### Next Session
Session 3.2: Runtime Remapping & Hot-Reload (config.toml watching, IPC commands, validation)

---

## Session: 2026-03-15 — Phase 3: Session 3.2

### What Happened

#### Session 3.2: Runtime Remapping & Hot-Reload
1. **Config file watcher** using `notify` crate (v7, FSEvents on macOS): watches config directory, filters for config.toml changes, bridges to tokio via `tokio::sync::mpsc::unbounded_channel`. 250ms leading-edge debounce prevents rapid reloads. Self-write suppression prevents watcher from reverting IPC-set remaps after `save_config()`.
2. **Hot-reload key remapping**: `handle_config_change()` in Service re-reads config.toml, validates, and pushes updated `KeyRemapConfig` through `Emulation::update_key_remap()` → `EmulationRequest` → `ProxyRequest` → live `KeyRemapEngine` rebuild in `do_emulation_session()`. No restart needed.
3. **IPC commands**: Added `SetKeyRemap { modifiers, keys }`, `GetKeyRemap`, `ResetKeyRemap` to `FrontendRequest`; `KeyRemapState { modifiers, keys }` to `FrontendEvent`. String-based HashMap format matching TOML config. `SetKeyRemap` persists to `config_toml.key_remap` so `save_config()` includes changes.
4. **`--remap` CLI flag**: `--remap ctrl=cmd --remap KeyCapsLock=KeyEsc` — supports both modifier roles and scancode names. Applied on top of config file, re-applied on hot-reload. CLI overrides reflected in IPC `GetKeyRemap` responses via `merge_cli_remap_strings()`.
5. **Validation**: `KeyRemapConfig::validate()` detects self-remaps (no-ops), duplicate modifier sources (conflicts), chains in both modifier and key remapping. Warnings logged on load; valid configs still applied.
6. **Emulation update channel**: `UpdateKeyRemap(KeyRemapConfig)` added to `EmulationRequest`, `ProxyRequest`. Handled in all code paths: active session (live engine rebuild), inactive wait loop (stored for next `do_emulation()` cycle), and termination wait (ignored).

### Plan Divergences
- **No trailing-edge debounce**: Used leading-edge debounce with self-write suppression instead of a trailing timer. Simpler, and macOS FSEvents coalesces events well enough. Could add trailing retry if partial-file reads become an issue with specific editors.
- **IPC key remap uses string maps, not KeyRemapConfig directly**: `lan-mouse-ipc` crate doesn't depend on binary crate types. Used `HashMap<String, String>` format matching TOML config for both modifiers and keys.

### Cross-Verification
- **Code reviewer (sonnet)**: 0 critical, 3 warnings, 4 suggestions. All 3 warnings fixed:
  1. [FIXED] IPC-set remaps not persisted to config_toml — added `set_key_remap_toml()` and called from `set_key_remap()`
  2. [FIXED] Watcher save-loop reverting IPC changes — `save_config()` now sets `last_config_reload = Instant::now()` to suppress watcher
  3. [FIXED] CLI overrides not in IPC state — `merge_cli_remap_strings()` includes `--remap` entries in IPC responses

### Verification
- `cargo build --no-default-features` ✓
- `cargo build --no-default-features --features macos_vhid` ✓
- `cargo fmt --check` ✓
- `cargo clippy --no-default-features -- -D warnings` ✓
- `cargo test --no-default-features` ✓ (23 tests: 17 keymap + 6 coalescer)

### Errors
None.

### Files Changed
| File | Change |
|------|--------|
| `Cargo.toml` | Added `notify = "7"` dependency |
| `Cargo.lock` | Updated lockfile for notify + transitive deps |
| `lan-mouse-ipc/src/lib.rs` | Added SetKeyRemap, GetKeyRemap, ResetKeyRemap requests + KeyRemapState event |
| `src/keymap.rs` | Added `ModifierRole::to_config_str()`, `KeyRemapConfig::validate()`, 8 new tests |
| `src/config.rs` | Added `--remap` CLI flag, `parse_key_remap_toml()`, `apply_remap_override()`, `parse_remap_strings()`, `reload_key_remap_from_disk()`, `key_remap_strings()`, `set_key_remap_toml()`, `merge_cli_remap_strings()` |
| `src/emulation.rs` | Added `UpdateKeyRemap` to EmulationRequest/ProxyRequest, `update_key_remap()` on Emulation/EmulationProxy, handlers in ListenTask, EmulationTask, wait loops |
| `src/service.rs` | Added notify file watcher, config_change channel, debounce, `handle_config_change()`, `set_key_remap()`, IPC handlers, self-write suppression in `save_config()` |

### Next Session
Session 3.3: Special Key Handling (fn/Globe, media keys, PrintScreen, CapsLock sync)

---

## Session: 2026-03-15 — Phase 3: Session 3.3

### What Happened

#### Session 3.3: Special Key Handling
1. **CGEventPost media keys**: Volume Up/Down/Mute via Mac virtual keycodes (0x48/0x49/0x4A) using standard `CGEvent::new_keyboard_event()`. Play/Pause, Next/Previous, Brightness Up/Down via NX system-defined events using raw CGEvent C API — `CGEventCreate` + `CGEventSetType(14)` + fields 131-133 for subtype/data1/data2.
2. **VirtualHID media keys**: All media keys via USB HID Consumer Control page (0x0C) with proper usage codes: Volume (0xE9/0xEA/0xE2), Play/Pause (0xCD), Next/Prev (0xB5/0xB6), Stop (0xB7), Brightness (0x006F/0x0070).
3. **fn/Globe key**: VirtualHID-only via Apple vendor page (0xFF01, usage 0x0003). CGEventPost cannot synthesize this — event is silently dropped if no VirtualHID backend.
4. **PrintScreen → Screenshot**: CGEventPost synthesizes Cmd+Shift+3 with modifier flags on `CGEvent::new_keyboard_event`. VirtualHID synthesizes full key sequence (GUI↓, Shift↓, 3↓, 3↑, Shift↑, GUI↑) with atomic readiness check.
5. **Context menu key**: Both backends synthesize right-click at current cursor position via `CGEvent::new_mouse_event(RightMouseDown/Up)`.
6. **CapsLock toggle**: Handled via normal Key event path (evdev 58 → keycode crate → Mac 0x39). Modifiers-based sync intentionally omitted to prevent double-toggle race.

### Plan Divergences
- **CapsLock Modifiers sync removed**: The task plan said "Handle CapsLock toggle state sync between machines." Initial implementation added sync from `Modifiers { locked }` events. Code review identified a critical double-toggle race: Key event toggles CapsLock, then Modifiers event arrives before `CGEventSourceFlagsState` updates, triggering a second toggle (net zero). Removed Modifiers-based sync — CapsLock key events handle the toggle correctly through the normal keycode mapping path.
- **Brightness NX_KEYTYPE values corrected**: Initially used 22/23 (keyboard illumination), code review caught this — correct values are 2/3 per Apple's `ev_keymap.h`.
- **No new files created**: All changes in existing backend files as specified by task plan.

### Cross-Verification
- **Code reviewer (sonnet)**: 2 critical, 5 warnings, 4 suggestions found.
  1. [FIXED] CRITICAL: Wrong NX_KEYTYPE brightness values (22/23 → 2/3)
  2. [FIXED] CRITICAL: CapsLock double-toggle from Key + Modifiers paths — removed Modifiers sync
  3. [FIXED] WARNING: VirtualHID screenshot partial sequence risk — check ready once before sequence
  4. [FIXED] WARNING: `send_vhid_key` unused after screenshot refactor — removed
  5. [NOTED] WARNING: EVDEV_KEY_STOPCD handled differently between backends — added comment
  6. [NOTED] WARNING: Context menu doesn't update click-state tracking — edge case, deferred
  7. [NOTED] WARNING: Undocumented NX event fields 131-133 — comment in code, no public API alternative

### Verification
- `cargo build --no-default-features` ✓
- `cargo build --no-default-features --features macos_vhid` ✓
- `cargo fmt --check` ✓
- `cargo clippy --no-default-features -- -D warnings` ✓
- `cargo test --no-default-features` ✓ (23 tests: 17 keymap + 6 coalescer)

### Errors
None.

### Files Changed
| File | Change |
|------|--------|
| `input-emulation/src/macos.rs` | Added special key handling: NX media key injection via raw CGEvent C API, volume via Mac keycodes, PrintScreen synthesis, context menu right-click, evdev scancode constants |
| `input-emulation/src/macos_vhid.rs` | Added special key handling: Consumer Control page (0x0C) for media keys, Apple vendor page (0xFF01) for fn/Globe, PrintScreen synthesis via key sequence, context menu right-click, HID usage constants |

### Next Session
Begin Phase 4: Clipboard Enhancement (Session 4.1: Image Clipboard)

---

## Session: 2026-03-15 — Phase 3 Cross-Review Audit

### What Happened
Phase-level quality gate cross-review of all Phase 3 changes (9c3bc3a..ed38e11, 1911 insertions across 12 files).

### Review Sources
1. **Code Reviewer Agent (opus)**: Comprehensive review — 0 critical, 5 warnings, 6 info
2. **GemSuite MCP (gemini_reason)**: Algorithm verification — bitmask remapping circular swap correctness confirmed across all 5 scenarios

### Findings & Fixes

#### [FIXED] WARNING: `unwrap()` on `get_mouse_location()` can panic
- **Files**: `input-emulation/src/macos.rs:507`, `input-emulation/src/macos_vhid.rs:482`
- **Issue**: Button event handler used `.unwrap()` on `get_mouse_location()`, which can return `None` if CGEvent source is in bad state. Contrasts with correct `match`/`let-else` patterns used elsewhere in the same files.
- **Fix**: Replaced with `let Some(location) = ... else { return Ok(()); }` pattern.

#### [FIXED] WARNING: Pressed modifier state lost on hot-reload
- **Files**: `src/keymap.rs` (new `drain_pressed()`), `src/emulation.rs:431-446`
- **Issue**: When `UpdateKeyRemap` rebuilds the engine, `pressed_modifiers` was cleared without synthesizing release events. If a user held Ctrl (mapped to Meta) during hot-reload that changed Ctrl→Alt, the release would emit Alt instead of Meta — leaving Meta stuck.
- **Fix**: Added `KeyRemapEngine::drain_pressed()` method. UpdateKeyRemap handler now drains old pressed state and synthesizes release events using OLD mapping before rebuilding.

#### [FIXED] WARNING: `wait_for_termination` silently drops `UpdateKeyRemap`
- **Files**: `src/emulation.rs:473-484` (signature change), two call sites updated
- **Issue**: During emulation backend initialization, `UpdateKeyRemap` messages were silently discarded. If IPC `SetKeyRemap` arrived during startup, the config was lost.
- **Fix**: Changed `wait_for_termination` to accept `&mut KeyRemapConfig` parameter and store incoming config updates.

#### [NOTED] WARNING: Debounce race between save_config and external edits
- **File**: `src/service.rs:334,647`
- **Issue**: 250ms debounce window after `save_config()` can suppress a coincident external edit.
- **Status**: Acknowledged, deferred. Edge case too narrow to warrant added complexity (boolean flag or content hashing). External edit will be picked up on next watcher event.

#### [NOTED] WARNING: `merge_cli_remap_strings` classification ambiguity
- **File**: `src/config.rs:603-606`
- **Issue**: If only one side of a `--remap` entry parses as a modifier role, it falls through to key remap and logs a confusing "unknown key name" error.
- **Status**: Acknowledged, deferred. Behavior is correct, only error message path is suboptimal.

### Verification
- `cargo build --no-default-features` ✓
- `cargo build --no-default-features --features macos_vhid` ✓
- `cargo fmt --check` ✓
- `cargo clippy --no-default-features -- -D warnings` ✓
- `cargo test --no-default-features` ✓ (23 tests)

### Files Changed
| File | Change |
|------|--------|
| `input-emulation/src/macos.rs` | Replaced `unwrap()` with graceful `let-else` on `get_mouse_location()` in Button handler |
| `input-emulation/src/macos_vhid.rs` | Same `unwrap()` fix in Button handler |
| `src/keymap.rs` | Added `drain_pressed()` method to `KeyRemapEngine` |
| `src/emulation.rs` | Added `KeyboardEvent` import; UpdateKeyRemap handler drains+releases old pressed state; `wait_for_termination` stores UpdateKeyRemap instead of discarding |
| `.claude/plans/.../progress.md` | This entry |
