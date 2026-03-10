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

        // DKEvent uses page 0x07 (Keyboard/Keypad) for regular keys
        // and page 0xFF (vendor-defined) for some special keys
        let mut event = kdk::DKEvent {
            value: if state > 0 { 1 } else { 0 },
            page: 0x07, // Keyboard/Keypad usage page
            code: hid_usage,
        };

        let result = kdk::send_key(&mut event as *mut kdk::DKEvent);
        if result != 0 {
            log::warn!("VirtualHID send_key failed: {result}");
        }
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
                    // Modifier state is tracked internally by VirtualHID
                    // through individual key press/release events
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
                        let (new_x, new_y) =
                            clamp_to_screen_space(location.x, location.y, dx, dy);
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
                    PointerEvent::Button {
                        button,
                        state,
                        ..
                    } => {
                        let cg_button_number: Option<i64> = match button {
                            BTN_BACK => Some(3),
                            BTN_FORWARD => Some(4),
                            _ => None,
                        };
                        let (event_type, mouse_button) = match (button, state) {
                            (BTN_LEFT, 1) => (CGEventType::LeftMouseDown, CGMouseButton::Left),
                            (BTN_LEFT, 0) => (CGEventType::LeftMouseUp, CGMouseButton::Left),
                            (BTN_RIGHT, 1) => {
                                (CGEventType::RightMouseDown, CGMouseButton::Right)
                            }
                            (BTN_RIGHT, 0) => (CGEventType::RightMouseUp, CGMouseButton::Right),
                            (BTN_MIDDLE, 1) => {
                                (CGEventType::OtherMouseDown, CGMouseButton::Center)
                            }
                            (BTN_MIDDLE, 0) => {
                                (CGEventType::OtherMouseUp, CGMouseButton::Center)
                            }
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
                                && self
                                    .previous_button_click
                                    .is_some_and(|i| {
                                        i.elapsed()
                                            < std::time::Duration::from_millis(500)
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
