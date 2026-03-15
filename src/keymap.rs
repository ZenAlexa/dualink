//! Modifier-aware key remapping engine.
//!
//! Transforms keyboard events based on modifier role remapping (e.g., Ctrl→Cmd)
//! and simple key-to-key remapping (e.g., CapsLock→Escape). Tracks modifier
//! key press/release state to handle stickiness correctly.

use std::collections::{HashMap, HashSet};

use input_event::{
    Event, KeyboardEvent,
    scancode::Linux::{
        KeyLeftAlt, KeyLeftCtrl, KeyLeftMeta, KeyLeftShift, KeyRightCtrl, KeyRightShift,
        KeyRightalt, KeyRightmeta,
    },
};

/// Modifier roles for user-friendly configuration.
///
/// Each role maps to a pair of left/right Linux scancodes and an XMods bitmask.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ModifierRole {
    Ctrl,
    Shift,
    Alt,
    Meta,
}

impl ModifierRole {
    /// Parse from config string (case-insensitive).
    ///
    /// Accepts common aliases: "ctrl"/"control", "alt"/"option",
    /// "meta"/"cmd"/"command"/"win"/"super".
    pub fn from_config_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "ctrl" | "control" => Some(Self::Ctrl),
            "shift" => Some(Self::Shift),
            "alt" | "option" => Some(Self::Alt),
            "meta" | "cmd" | "command" | "win" | "super" => Some(Self::Meta),
            _ => None,
        }
    }

    /// Return (left, right) Linux scancodes for this modifier role.
    fn scancodes(self) -> (u32, u32) {
        match self {
            Self::Ctrl => (KeyLeftCtrl as u32, KeyRightCtrl as u32),
            Self::Shift => (KeyLeftShift as u32, KeyRightShift as u32),
            Self::Alt => (KeyLeftAlt as u32, KeyRightalt as u32),
            Self::Meta => (KeyLeftMeta as u32, KeyRightmeta as u32),
        }
    }

    /// XMods bitmask for this modifier role (X11 convention).
    fn xmod_bit(self) -> u32 {
        match self {
            Self::Shift => 1 << 0,
            Self::Ctrl => 1 << 2,
            Self::Alt => 1 << 3,
            Self::Meta => 1 << 6,
        }
    }

    /// All modifier roles.
    fn all() -> &'static [ModifierRole] {
        &[Self::Ctrl, Self::Shift, Self::Alt, Self::Meta]
    }
}

/// Configuration for the key remapping engine.
#[derive(Debug, Clone, Default)]
pub struct KeyRemapConfig {
    /// Modifier role remapping (source role → target role).
    pub modifier_remap: Vec<(ModifierRole, ModifierRole)>,
    /// Simple key-to-key remapping (scancode → scancode).
    pub key_remap: HashMap<u32, u32>,
}

impl KeyRemapConfig {
    /// Whether any remapping is configured.
    pub fn is_empty(&self) -> bool {
        self.modifier_remap.is_empty() && self.key_remap.is_empty()
    }

    /// Total number of active scancode mappings.
    pub fn mapping_count(&self) -> usize {
        self.modifier_remap.len() * 2 + self.key_remap.len()
    }
}

/// Stateful key remapping engine with modifier awareness.
///
/// Tracks which modifier keys are physically pressed and what they were
/// remapped to, ensuring correct release even if configuration changes.
pub struct KeyRemapEngine {
    /// Merged scancode-to-scancode map (modifiers + keys).
    scancode_map: HashMap<u32, u32>,
    /// Modifier bit remap pairs for Modifiers events: (source_bit, target_bit).
    modifier_bit_remap: Vec<(u32, u32)>,
    /// OR of all source bits in modifier_bit_remap (for clearing).
    source_bits: u32,
    /// Physical modifier scancodes currently pressed → remapped scancode.
    pressed_modifiers: HashMap<u32, u32>,
    /// Set of all modifier key scancodes (for state tracking).
    modifier_scancodes: HashSet<u32>,
}

impl KeyRemapEngine {
    /// Create a new engine from configuration.
    pub fn new(config: &KeyRemapConfig) -> Self {
        let mut scancode_map = HashMap::new();
        let mut modifier_bit_remap = Vec::new();
        let mut source_bits = 0u32;

        // Build scancode map and bit remap from modifier config
        for &(src_role, dst_role) in &config.modifier_remap {
            if src_role == dst_role {
                continue;
            }
            let (src_left, src_right) = src_role.scancodes();
            let (dst_left, dst_right) = dst_role.scancodes();
            scancode_map.insert(src_left, dst_left);
            scancode_map.insert(src_right, dst_right);

            modifier_bit_remap.push((src_role.xmod_bit(), dst_role.xmod_bit()));
            source_bits |= src_role.xmod_bit();
        }

        // Add simple key remaps (modifier remaps take precedence)
        for (&src, &dst) in &config.key_remap {
            scancode_map.entry(src).or_insert(dst);
        }

        // Collect all modifier scancodes for state tracking
        let modifier_scancodes: HashSet<u32> = ModifierRole::all()
            .iter()
            .flat_map(|role| {
                let (l, r) = role.scancodes();
                [l, r]
            })
            .collect();

        Self {
            scancode_map,
            modifier_bit_remap,
            source_bits,
            pressed_modifiers: HashMap::new(),
            modifier_scancodes,
        }
    }

    /// Whether any remapping is active.
    pub fn is_active(&self) -> bool {
        !self.scancode_map.is_empty()
    }

    /// Clear all pressed modifier state (e.g., on client disconnect).
    pub fn reset(&mut self) {
        self.pressed_modifiers.clear();
    }

    /// Remap a single event, updating internal modifier state.
    pub fn remap_event(&mut self, event: Event) -> Event {
        match event {
            Event::Keyboard(KeyboardEvent::Key { time, key, state }) => {
                let remapped = self.remap_key(key, state);
                Event::Keyboard(KeyboardEvent::Key {
                    time,
                    key: remapped,
                    state,
                })
            }
            Event::Keyboard(KeyboardEvent::Modifiers {
                depressed,
                latched,
                locked,
                group,
            }) => Event::Keyboard(KeyboardEvent::Modifiers {
                depressed: self.remap_modifier_bits(depressed),
                latched,
                locked,
                group,
            }),
            other => other,
        }
    }

    /// Remap a key scancode, tracking modifier state for stickiness.
    fn remap_key(&mut self, key: u32, state: u8) -> u32 {
        let is_press = state != 0;
        let is_modifier = self.modifier_scancodes.contains(&key);

        if is_press {
            let remapped = self.scancode_map.get(&key).copied().unwrap_or(key);
            if is_modifier {
                self.pressed_modifiers.insert(key, remapped);
            }
            remapped
        } else {
            // On release, use tracked value for stickiness
            if is_modifier {
                if let Some(remapped) = self.pressed_modifiers.remove(&key) {
                    return remapped;
                }
            }
            self.scancode_map.get(&key).copied().unwrap_or(key)
        }
    }

    /// Remap modifier bitmask in Modifiers events.
    ///
    /// Handles circular swaps correctly by reading all source bits from
    /// the original value before clearing and setting target bits.
    fn remap_modifier_bits(&self, depressed: u32) -> u32 {
        if self.modifier_bit_remap.is_empty() {
            return depressed;
        }

        // Clear only source bits from the result, then set destination bits
        // from the *original* depressed value. Reading originals before writing
        // is what makes circular swaps (e.g., Ctrl↔Meta) correct.
        let mut result = depressed & !self.source_bits;

        // Set destination bits based on original source state
        for &(src_bit, dst_bit) in &self.modifier_bit_remap {
            if depressed & src_bit != 0 {
                result |= dst_bit;
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use input_event::PointerEvent;

    const LEFT_CTRL: u32 = KeyLeftCtrl as u32;
    const RIGHT_CTRL: u32 = KeyRightCtrl as u32;
    const LEFT_META: u32 = KeyLeftMeta as u32;
    const RIGHT_META: u32 = KeyRightmeta as u32;
    const CAPS_LOCK: u32 = input_event::scancode::Linux::KeyCapsLock as u32;
    const KEY_ESC: u32 = input_event::scancode::Linux::KeyEsc as u32;

    fn key_press(key: u32) -> Event {
        Event::Keyboard(KeyboardEvent::Key {
            time: 0,
            key,
            state: 1,
        })
    }

    fn key_release(key: u32) -> Event {
        Event::Keyboard(KeyboardEvent::Key {
            time: 0,
            key,
            state: 0,
        })
    }

    fn modifiers_event(depressed: u32) -> Event {
        Event::Keyboard(KeyboardEvent::Modifiers {
            depressed,
            latched: 0,
            locked: 0,
            group: 0,
        })
    }

    #[test]
    fn empty_config_is_inactive() {
        let config = KeyRemapConfig::default();
        let engine = KeyRemapEngine::new(&config);
        assert!(!engine.is_active());
        assert!(config.is_empty());
    }

    #[test]
    fn simple_key_remap() {
        let config = KeyRemapConfig {
            modifier_remap: vec![],
            key_remap: HashMap::from([(CAPS_LOCK, KEY_ESC)]),
        };
        let mut engine = KeyRemapEngine::new(&config);
        assert_eq!(engine.remap_event(key_press(CAPS_LOCK)), key_press(KEY_ESC));
        assert_eq!(
            engine.remap_event(key_release(CAPS_LOCK)),
            key_release(KEY_ESC)
        );
    }

    #[test]
    fn modifier_remap_ctrl_to_meta() {
        let config = KeyRemapConfig {
            modifier_remap: vec![(ModifierRole::Ctrl, ModifierRole::Meta)],
            key_remap: HashMap::new(),
        };
        let mut engine = KeyRemapEngine::new(&config);

        assert_eq!(
            engine.remap_event(key_press(LEFT_CTRL)),
            key_press(LEFT_META)
        );
        assert_eq!(
            engine.remap_event(key_release(LEFT_CTRL)),
            key_release(LEFT_META)
        );
        assert_eq!(
            engine.remap_event(key_press(RIGHT_CTRL)),
            key_press(RIGHT_META)
        );
        assert_eq!(
            engine.remap_event(key_release(RIGHT_CTRL)),
            key_release(RIGHT_META)
        );
    }

    #[test]
    fn modifier_stickiness_on_release() {
        let config = KeyRemapConfig {
            modifier_remap: vec![(ModifierRole::Ctrl, ModifierRole::Meta)],
            key_remap: HashMap::new(),
        };
        let mut engine = KeyRemapEngine::new(&config);

        // Press tracks the remapped value
        assert_eq!(
            engine.remap_event(key_press(LEFT_CTRL)),
            key_press(LEFT_META)
        );
        assert_eq!(engine.pressed_modifiers.get(&LEFT_CTRL), Some(&LEFT_META));

        // Release uses tracked value (sticky)
        assert_eq!(
            engine.remap_event(key_release(LEFT_CTRL)),
            key_release(LEFT_META)
        );
        assert!(engine.pressed_modifiers.is_empty());
    }

    #[test]
    fn circular_modifier_swap() {
        let config = KeyRemapConfig {
            modifier_remap: vec![
                (ModifierRole::Ctrl, ModifierRole::Meta),
                (ModifierRole::Meta, ModifierRole::Ctrl),
            ],
            key_remap: HashMap::new(),
        };
        let mut engine = KeyRemapEngine::new(&config);

        assert_eq!(
            engine.remap_event(key_press(LEFT_CTRL)),
            key_press(LEFT_META)
        );
        assert_eq!(
            engine.remap_event(key_press(LEFT_META)),
            key_press(LEFT_CTRL)
        );
        assert_eq!(
            engine.remap_event(key_release(LEFT_CTRL)),
            key_release(LEFT_META)
        );
        assert_eq!(
            engine.remap_event(key_release(LEFT_META)),
            key_release(LEFT_CTRL)
        );
    }

    #[test]
    fn modifier_bits_remap() {
        let config = KeyRemapConfig {
            modifier_remap: vec![(ModifierRole::Ctrl, ModifierRole::Meta)],
            key_remap: HashMap::new(),
        };
        let mut engine = KeyRemapEngine::new(&config);

        let ctrl_bit = ModifierRole::Ctrl.xmod_bit();
        let meta_bit = ModifierRole::Meta.xmod_bit();

        // Ctrl depressed → becomes Meta
        assert_eq!(engine.remap_modifier_bits(ctrl_bit), meta_bit);
        // Meta depressed (not a source) → preserved
        assert_eq!(engine.remap_modifier_bits(meta_bit), meta_bit);
        // No modifiers → no change
        assert_eq!(engine.remap_modifier_bits(0), 0);
        // End-to-end through remap_event
        assert_eq!(
            engine.remap_event(modifiers_event(ctrl_bit)),
            modifiers_event(meta_bit)
        );
    }

    #[test]
    fn modifier_bits_circular_swap() {
        let config = KeyRemapConfig {
            modifier_remap: vec![
                (ModifierRole::Ctrl, ModifierRole::Meta),
                (ModifierRole::Meta, ModifierRole::Ctrl),
            ],
            key_remap: HashMap::new(),
        };
        let engine = KeyRemapEngine::new(&config);

        let ctrl_bit = ModifierRole::Ctrl.xmod_bit();
        let meta_bit = ModifierRole::Meta.xmod_bit();

        assert_eq!(engine.remap_modifier_bits(ctrl_bit), meta_bit);
        assert_eq!(engine.remap_modifier_bits(meta_bit), ctrl_bit);
        assert_eq!(
            engine.remap_modifier_bits(ctrl_bit | meta_bit),
            ctrl_bit | meta_bit
        );
    }

    #[test]
    fn reset_clears_pressed_state() {
        let config = KeyRemapConfig {
            modifier_remap: vec![(ModifierRole::Ctrl, ModifierRole::Meta)],
            key_remap: HashMap::new(),
        };
        let mut engine = KeyRemapEngine::new(&config);

        engine.remap_event(key_press(LEFT_CTRL));
        assert!(!engine.pressed_modifiers.is_empty());

        engine.reset();
        assert!(engine.pressed_modifiers.is_empty());
    }

    #[test]
    fn non_keyboard_events_pass_through() {
        let config = KeyRemapConfig {
            modifier_remap: vec![(ModifierRole::Ctrl, ModifierRole::Meta)],
            key_remap: HashMap::from([(CAPS_LOCK, KEY_ESC)]),
        };
        let mut engine = KeyRemapEngine::new(&config);

        let motion = Event::Pointer(PointerEvent::Motion {
            time: 0,
            dx: 1.0,
            dy: 2.0,
        });
        assert_eq!(engine.remap_event(motion), motion);

        let button = Event::Pointer(PointerEvent::Button {
            time: 0,
            button: 0x110,
            state: 1,
        });
        assert_eq!(engine.remap_event(button), button);
    }
}
