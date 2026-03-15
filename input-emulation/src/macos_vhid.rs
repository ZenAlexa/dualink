//! VirtualHID emulation backend for macOS.
//!
//! Uses Karabiner-DriverKit-VirtualHIDDevice for keyboard input injection,
//! making keyboard events visible to Karabiner-Elements for processing.
//! Mouse events are forwarded via CGEventPost (Karabiner does not typically
//! remap mouse events, so this hybrid approach gives the best compatibility).

use super::{Emulation, EmulationHandle, error::EmulationError};
use async_trait::async_trait;
use core_graphics::base::CGFloat;
use core_graphics::display::{
    CGDirectDisplayID, CGDisplayBounds, CGGetDisplaysWithRect, CGPoint, CGRect, CGSize,
};
use core_graphics::event::{
    CGEvent, CGEventTapLocation, CGEventType, CGMouseButton, EventField, ScrollEventUnit,
};
use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};
use input_event::{
    BTN_BACK, BTN_FORWARD, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, Event, KeyboardEvent, PointerEvent,
};
use karabiner_driverkit as kdk;
use keycode::{KeyMap, KeyMapping};
use std::collections::HashSet;

use super::error::MacOSEmulationCreationError;

const VHID_DAEMON_SOCKET_DIR: &str =
    "/Library/Application Support/org.pqrs/tmp/rootonly/vhidd_server";

// HID Usage Page constants
const HID_USAGE_PAGE_KEYBOARD: u32 = 0x07;
const HID_USAGE_PAGE_CONSUMER: u32 = 0x0C;
const HID_USAGE_PAGE_APPLE_VENDOR: u32 = 0xFF01;

// HID Consumer Control Page (0x0C) usage codes
const HID_CONSUMER_VOLUME_UP: u32 = 0xE9;
const HID_CONSUMER_VOLUME_DOWN: u32 = 0xEA;
const HID_CONSUMER_MUTE: u32 = 0xE2;
const HID_CONSUMER_PLAY_PAUSE: u32 = 0xCD;
const HID_CONSUMER_SCAN_NEXT_TRACK: u32 = 0xB5;
const HID_CONSUMER_SCAN_PREVIOUS_TRACK: u32 = 0xB6;
const HID_CONSUMER_STOP: u32 = 0xB7;
const HID_CONSUMER_BRIGHTNESS_UP: u32 = 0x006F;
const HID_CONSUMER_BRIGHTNESS_DOWN: u32 = 0x0070;

// Apple vendor-defined Globe/fn key (page 0xFF01)
const HID_APPLE_FN_GLOBE: u32 = 0x0003;

// USB HID Keyboard page usage codes for key synthesis
const HID_KEY_LEFT_GUI: u32 = 0xE3;
const HID_KEY_LEFT_SHIFT: u32 = 0xE1;
const HID_KEY_3: u32 = 0x20;

// Linux evdev scancodes for special keys
const EVDEV_KEY_SYSRQ: u32 = 99;
const EVDEV_KEY_MUTE: u32 = 113;
const EVDEV_KEY_VOLUME_DOWN: u32 = 114;
const EVDEV_KEY_VOLUME_UP: u32 = 115;
const EVDEV_KEY_MENU: u32 = 139;
const EVDEV_KEY_NEXTSONG: u32 = 163;
const EVDEV_KEY_PLAYPAUSE: u32 = 164;
const EVDEV_KEY_PREVIOUSSONG: u32 = 165;
const EVDEV_KEY_STOPCD: u32 = 166;
const EVDEV_KEY_BRIGHTNESS_DOWN: u32 = 224;
const EVDEV_KEY_BRIGHTNESS_UP: u32 = 225;
const EVDEV_KEY_FN: u32 = 464;

pub(crate) struct VirtualHIDEmulation {
    /// CGEventSource for mouse events (forwarded via CGEventPost)
    event_source: CGEventSource,
    /// tracked pressed mouse buttons
    pressed_buttons: HashSet<u32>,
    /// previously pressed button (for double-click tracking)
    previous_button: Option<u32>,
    /// timestamp of previous click
    previous_button_click: Option<std::time::Instant>,
    /// click state for multi-click
    button_click_state: i64,
    /// whether the VirtualHID keyboard sink is active
    vhid_ready: bool,
}

unsafe impl Send for VirtualHIDEmulation {}

impl VirtualHIDEmulation {
    pub(crate) fn new() -> Result<Self, MacOSEmulationCreationError> {
        // Check if Karabiner VirtualHID daemon is available
        if !std::path::Path::new(VHID_DAEMON_SOCKET_DIR).exists() {
            return Err(MacOSEmulationCreationError::EventSourceCreation);
        }

        // Check if the DriverKit extension is activated
        if !kdk::driver_activated() {
            log::warn!("Karabiner VirtualHID driver not activated");
            return Err(MacOSEmulationCreationError::EventSourceCreation);
        }

        let event_source = CGEventSource::new(CGEventSourceStateID::CombinedSessionState)
            .map_err(|_| MacOSEmulationCreationError::EventSourceCreation)?;

        let vhid_ready = kdk::is_sink_ready();
        if !vhid_ready {
            log::warn!("VirtualHID keyboard sink not ready, will retry");
        }

        log::info!("VirtualHID emulation backend initialized");
        Ok(Self {
            event_source,
            pressed_buttons: HashSet::new(),
            previous_button: None,
            previous_button_click: None,
            button_click_state: 0,
            vhid_ready,
        })
    }

    fn get_mouse_location(&self) -> Option<CGPoint> {
        let event: CGEvent = CGEvent::new(self.event_source.clone()).ok()?;
        Some(event.location())
    }

    /// Send a keyboard event through VirtualHID
    fn send_keyboard_event(&mut self, key: u32, state: u8) {
        // Try special key handling first (media, screenshot, fn/Globe, etc.)
        if self.try_special_key(key, state) {
            return;
        }

        if !self.vhid_ready {
            self.vhid_ready = kdk::is_sink_ready();
            if !self.vhid_ready {
                log::warn!("VirtualHID sink not ready, dropping keyboard event");
                return;
            }
        }

        // Convert evdev scancode to USB HID usage
        let hid_usage = match evdev_to_hid_usage(key) {
            Some(u) => u,
            None => {
                log::debug!("no HID usage mapping for evdev key {key}");
                return;
            }
        };

        let mut event = kdk::DKEvent {
            value: if state > 0 { 1 } else { 0 },
            page: HID_USAGE_PAGE_KEYBOARD,
            code: hid_usage,
        };

        let result = kdk::send_key(&mut event as *mut kdk::DKEvent);
        if result != 0 {
            log::warn!("VirtualHID send_key failed: {result}");
        }
    }

    /// Send a key event on the HID Consumer Control page (media keys, brightness).
    fn send_consumer_key(&mut self, usage: u32, state: u8) {
        if !self.ensure_vhid_ready() {
            return;
        }
        let mut event = kdk::DKEvent {
            value: if state > 0 { 1 } else { 0 },
            page: HID_USAGE_PAGE_CONSUMER,
            code: usage,
        };
        let result = kdk::send_key(&mut event as *mut kdk::DKEvent);
        if result != 0 {
            log::warn!("VirtualHID send_key (consumer) failed: {result}");
        }
    }

    /// Send a key event on the Apple vendor-defined page (fn/Globe key).
    fn send_apple_vendor_key(&mut self, usage: u32, state: u8) {
        if !self.ensure_vhid_ready() {
            return;
        }
        let mut event = kdk::DKEvent {
            value: if state > 0 { 1 } else { 0 },
            page: HID_USAGE_PAGE_APPLE_VENDOR,
            code: usage,
        };
        let result = kdk::send_key(&mut event as *mut kdk::DKEvent);
        if result != 0 {
            log::warn!("VirtualHID send_key (apple vendor) failed: {result}");
        }
    }

    /// Check and lazily initialize VirtualHID sink readiness.
    fn ensure_vhid_ready(&mut self) -> bool {
        if !self.vhid_ready {
            self.vhid_ready = kdk::is_sink_ready();
            if !self.vhid_ready {
                log::warn!("VirtualHID sink not ready, dropping key event");
                return false;
            }
        }
        true
    }

    /// Try to handle a special key that the keycode crate cannot map.
    /// Returns true if the key was handled.
    fn try_special_key(&mut self, key: u32, state: u8) -> bool {
        match key {
            // Volume: HID Consumer Control page
            EVDEV_KEY_VOLUME_UP => {
                self.send_consumer_key(HID_CONSUMER_VOLUME_UP, state);
                true
            }
            EVDEV_KEY_VOLUME_DOWN => {
                self.send_consumer_key(HID_CONSUMER_VOLUME_DOWN, state);
                true
            }
            EVDEV_KEY_MUTE => {
                self.send_consumer_key(HID_CONSUMER_MUTE, state);
                true
            }
            // Media transport: HID Consumer Control page
            EVDEV_KEY_PLAYPAUSE => {
                self.send_consumer_key(HID_CONSUMER_PLAY_PAUSE, state);
                true
            }
            EVDEV_KEY_NEXTSONG => {
                self.send_consumer_key(HID_CONSUMER_SCAN_NEXT_TRACK, state);
                true
            }
            EVDEV_KEY_PREVIOUSSONG => {
                self.send_consumer_key(HID_CONSUMER_SCAN_PREVIOUS_TRACK, state);
                true
            }
            EVDEV_KEY_STOPCD => {
                self.send_consumer_key(HID_CONSUMER_STOP, state);
                true
            }
            // Brightness: HID Consumer Control page
            EVDEV_KEY_BRIGHTNESS_UP => {
                self.send_consumer_key(HID_CONSUMER_BRIGHTNESS_UP, state);
                true
            }
            EVDEV_KEY_BRIGHTNESS_DOWN => {
                self.send_consumer_key(HID_CONSUMER_BRIGHTNESS_DOWN, state);
                true
            }
            // fn/Globe key: Apple vendor page (VirtualHID-only — CGEventPost cannot
            // synthesize this key; the CGEventPost backend silently drops it)
            EVDEV_KEY_FN => {
                self.send_apple_vendor_key(HID_APPLE_FN_GLOBE, state);
                true
            }
            // PrintScreen → macOS screenshot (Cmd+Shift+3) via VirtualHID
            EVDEV_KEY_SYSRQ => {
                if state == 1 {
                    self.synthesize_screenshot();
                }
                true
            }
            // Context menu → right-click at current cursor position
            EVDEV_KEY_MENU => {
                if state == 1 {
                    self.synthesize_context_menu();
                }
                true
            }
            _ => false,
        }
    }

    /// Synthesize Cmd+Shift+3 for macOS screenshot via VirtualHID key events.
    fn synthesize_screenshot(&mut self) {
        // Check readiness once before the multi-key sequence to avoid
        // stuck modifier keys if the sink becomes unavailable mid-sequence.
        if !self.ensure_vhid_ready() {
            return;
        }
        let send = |code: u32, value: u64| {
            let mut ev = kdk::DKEvent {
                value,
                page: HID_USAGE_PAGE_KEYBOARD,
                code,
            };
            let _ = kdk::send_key(&mut ev as *mut kdk::DKEvent);
        };
        send(HID_KEY_LEFT_GUI, 1);
        send(HID_KEY_LEFT_SHIFT, 1);
        send(HID_KEY_3, 1);
        send(HID_KEY_3, 0);
        send(HID_KEY_LEFT_SHIFT, 0);
        send(HID_KEY_LEFT_GUI, 0);
        log::debug!("synthesized screenshot (Cmd+Shift+3) via VirtualHID");
    }

    /// Synthesize a right-click at current cursor position for context menu.
    fn synthesize_context_menu(&self) {
        let Some(location) = self.get_mouse_location() else {
            log::warn!("could not get mouse location for context menu");
            return;
        };
        if let Ok(event) = CGEvent::new_mouse_event(
            self.event_source.clone(),
            CGEventType::RightMouseDown,
            location,
            CGMouseButton::Right,
        ) {
            event.post(CGEventTapLocation::HID);
        }
        if let Ok(event) = CGEvent::new_mouse_event(
            self.event_source.clone(),
            CGEventType::RightMouseUp,
            location,
            CGMouseButton::Right,
        ) {
            event.post(CGEventTapLocation::HID);
        }
        log::debug!("synthesized context menu (right-click)");
    }
}

/// Convert evdev scancode to USB HID Usage ID
fn evdev_to_hid_usage(evdev_key: u32) -> Option<u32> {
    // Use the keycode crate to map evdev → USB HID
    match KeyMap::from_key_mapping(KeyMapping::Evdev(evdev_key as u16)) {
        Ok(km) => Some(km.usb as u32),
        Err(_) => None,
    }
}

fn get_display_at_point(x: CGFloat, y: CGFloat) -> Option<CGDirectDisplayID> {
    let mut displays: [CGDirectDisplayID; 16] = [0; 16];
    let mut display_count: u32 = 0;
    let rect = CGRect::new(&CGPoint::new(x, y), &CGSize::new(0.0, 0.0));
    let error = unsafe {
        CGGetDisplaysWithRect(
            rect,
            1,
            displays.as_mut_ptr(),
            &mut display_count as *mut u32,
        )
    };
    if error != 0 || display_count == 0 {
        return None;
    }
    displays.first().copied()
}

fn get_display_bounds(display: CGDirectDisplayID) -> (CGFloat, CGFloat, CGFloat, CGFloat) {
    unsafe {
        let bounds = CGDisplayBounds(display);
        (
            bounds.origin.x,
            bounds.origin.y,
            bounds.origin.x + bounds.size.width,
            bounds.origin.y + bounds.size.height,
        )
    }
}

fn clamp_to_screen_space(
    current_x: CGFloat,
    current_y: CGFloat,
    dx: CGFloat,
    dy: CGFloat,
) -> (CGFloat, CGFloat) {
    let current_display = match get_display_at_point(current_x, current_y) {
        Some(d) => d,
        None => return (current_x, current_y),
    };
    let new_x = current_x + dx;
    let new_y = current_y + dy;
    let final_display = get_display_at_point(new_x, new_y).unwrap_or(current_display);
    let (min_x, min_y, max_x, max_y) = get_display_bounds(final_display);
    (
        new_x.clamp(min_x, max_x - 1.),
        new_y.clamp(min_y, max_y - 1.),
    )
}

/// Maps an evdev button code to CGEventType for drag events
fn drag_event_type(button: u32) -> CGEventType {
    match button {
        BTN_LEFT => CGEventType::LeftMouseDragged,
        BTN_RIGHT => CGEventType::RightMouseDragged,
        _ => CGEventType::OtherMouseDragged,
    }
}

#[async_trait]
impl Emulation for VirtualHIDEmulation {
    async fn consume(
        &mut self,
        event: Event,
        _handle: EmulationHandle,
    ) -> Result<(), EmulationError> {
        match event {
            Event::Keyboard(keyboard_event) => match keyboard_event {
                KeyboardEvent::Key { key, state, .. } => {
                    self.send_keyboard_event(key, state);
                }
                KeyboardEvent::Modifiers { .. } => {
                    // Modifier state is tracked internally by VirtualHID through
                    // individual key press/release events. CapsLock toggle is
                    // handled via Key events (evdev 58) in send_keyboard_event.
                    // We do NOT sync from Modifiers `locked` field to avoid a
                    // double-toggle race with the Key event path.
                }
            },
            Event::Pointer(pointer_event) => {
                // Mouse events go through CGEventPost (same as standard macOS backend)
                match pointer_event {
                    PointerEvent::Motion { dx, dy, .. } => {
                        let location = match self.get_mouse_location() {
                            Some(l) => l,
                            None => return Ok(()),
                        };
                        let (new_x, new_y) = clamp_to_screen_space(location.x, location.y, dx, dy);
                        let mouse_location = CGPoint::new(new_x, new_y);
                        let event_type = self
                            .pressed_buttons
                            .iter()
                            .next()
                            .map(|&btn| drag_event_type(btn))
                            .unwrap_or(CGEventType::MouseMoved);
                        if let Ok(event) = CGEvent::new_mouse_event(
                            self.event_source.clone(),
                            event_type,
                            mouse_location,
                            CGMouseButton::Left,
                        ) {
                            event.set_integer_value_field(
                                EventField::MOUSE_EVENT_DELTA_X,
                                dx as i64,
                            );
                            event.set_integer_value_field(
                                EventField::MOUSE_EVENT_DELTA_Y,
                                dy as i64,
                            );
                            event.post(CGEventTapLocation::HID);
                        }
                    }
                    PointerEvent::Button { button, state, .. } => {
                        let cg_button_number: Option<i64> = match button {
                            BTN_BACK => Some(3),
                            BTN_FORWARD => Some(4),
                            _ => None,
                        };
                        let (event_type, mouse_button) = match (button, state) {
                            (BTN_LEFT, 1) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
                            (BTN_LEFT, 0) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
                            (BTN_RIGHT, 1) => (CGEventType::RightMouseDown, CGMouseButton::Right),
                            (BTN_RIGHT, 0) => (CGEventType::RightMouseUp, CGMouseButton::Right),
                            (BTN_MIDDLE, 1) => (CGEventType::OtherMouseDown, CGMouseButton::Center),
                            (BTN_MIDDLE, 0) => (CGEventType::OtherMouseUp, CGMouseButton::Center),
                            (BTN_BACK, 1) | (BTN_FORWARD, 1) => {
                                (CGEventType::OtherMouseDown, CGMouseButton::Center)
                            }
                            (BTN_BACK, 0) | (BTN_FORWARD, 0) => {
                                (CGEventType::OtherMouseUp, CGMouseButton::Center)
                            }
                            _ => return Ok(()),
                        };

                        if state == 1 {
                            self.pressed_buttons.insert(button);
                        } else {
                            self.pressed_buttons.remove(&button);
                        }

                        if state == 1 {
                            if self.previous_button == Some(button)
                                && self.previous_button_click.is_some_and(|i| {
                                    i.elapsed() < std::time::Duration::from_millis(500)
                                })
                            {
                                self.button_click_state += 1;
                            } else {
                                self.button_click_state = 1;
                            }
                            self.previous_button = Some(button);
                            self.previous_button_click = Some(std::time::Instant::now());
                        }

                        let location = self.get_mouse_location().unwrap();
                        if let Ok(event) = CGEvent::new_mouse_event(
                            self.event_source.clone(),
                            event_type,
                            location,
                            mouse_button,
                        ) {
                            event.set_integer_value_field(
                                EventField::MOUSE_EVENT_CLICK_STATE,
                                self.button_click_state,
                            );
                            if let Some(btn_num) = cg_button_number {
                                event.set_integer_value_field(
                                    EventField::MOUSE_EVENT_BUTTON_NUMBER,
                                    btn_num,
                                );
                            }
                            event.post(CGEventTapLocation::HID);
                        }
                    }
                    PointerEvent::Axis { axis, value, .. } => {
                        let value = value as i32;
                        let (count, w1, w2, w3) = match axis {
                            0 => (1, value, 0, 0),
                            1 => (2, 0, value, 0),
                            _ => return Ok(()),
                        };
                        if let Ok(event) = CGEvent::new_scroll_event(
                            self.event_source.clone(),
                            ScrollEventUnit::PIXEL,
                            count,
                            w1,
                            w2,
                            w3,
                        ) {
                            event.post(CGEventTapLocation::HID);
                        }
                    }
                    PointerEvent::AxisDiscrete120 { axis, value } => {
                        const LINES_PER_STEP: i32 = 3;
                        let (count, w1, w2, w3) = match axis {
                            0 => (1, value / (120 / LINES_PER_STEP), 0, 0),
                            1 => (2, 0, value / (120 / LINES_PER_STEP), 0),
                            _ => return Ok(()),
                        };
                        if let Ok(event) = CGEvent::new_scroll_event(
                            self.event_source.clone(),
                            ScrollEventUnit::LINE,
                            count,
                            w1,
                            w2,
                            w3,
                        ) {
                            event.post(CGEventTapLocation::HID);
                        }
                    }
                }

                if !matches!(pointer_event, PointerEvent::Button { .. }) {
                    self.button_click_state = 0;
                }
            }
        }
        Ok(())
    }

    async fn create(&mut self, _handle: EmulationHandle) {}

    async fn destroy(&mut self, _handle: EmulationHandle) {}

    async fn terminate(&mut self) {
        // Release any grabbed devices
        kdk::release();
    }
}
