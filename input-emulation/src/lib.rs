use async_trait::async_trait;
use std::{
    collections::{HashMap, HashSet},
    fmt::Display,
};

use input_event::{Event, KeyboardEvent, PointerEvent};

pub use self::error::{EmulationCreationError, EmulationError, InputEmulationError};

/// Runtime mouse and scroll configuration applied before dispatch to backend.
#[derive(Clone, Debug)]
pub struct MouseConfig {
    /// Multiplier for pointer motion deltas (default 1.0).
    pub speed: f64,
    /// Multiplier for scroll events (default 1.0).
    pub scroll_speed: f64,
    /// Whether to invert scroll direction for natural scrolling.
    pub natural_scrolling: bool,
}

impl Default for MouseConfig {
    fn default() -> Self {
        Self {
            speed: 1.0,
            scroll_speed: 1.0,
            natural_scrolling: false,
        }
    }
}

#[cfg(windows)]
mod windows;

#[cfg(all(unix, feature = "x11", not(target_os = "macos")))]
mod x11;

#[cfg(all(unix, feature = "wlroots", not(target_os = "macos")))]
mod wlroots;

#[cfg(all(unix, feature = "remote_desktop_portal", not(target_os = "macos")))]
mod xdg_desktop_portal;

#[cfg(all(unix, feature = "libei", not(target_os = "macos")))]
mod libei;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(all(target_os = "macos", feature = "macos_vhid"))]
mod macos_vhid;

/// fallback input emulation (logs events)
mod dummy;
mod error;

pub type EmulationHandle = u64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    #[cfg(all(unix, feature = "wlroots", not(target_os = "macos")))]
    Wlroots,
    #[cfg(all(unix, feature = "libei", not(target_os = "macos")))]
    Libei,
    #[cfg(all(unix, feature = "remote_desktop_portal", not(target_os = "macos")))]
    Xdp,
    #[cfg(all(unix, feature = "x11", not(target_os = "macos")))]
    X11,
    #[cfg(windows)]
    Windows,
    #[cfg(target_os = "macos")]
    MacOs,
    #[cfg(all(target_os = "macos", feature = "macos_vhid"))]
    MacOsVHID,
    Dummy,
}

impl Display for Backend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(all(unix, feature = "wlroots", not(target_os = "macos")))]
            Backend::Wlroots => write!(f, "wlroots"),
            #[cfg(all(unix, feature = "libei", not(target_os = "macos")))]
            Backend::Libei => write!(f, "libei"),
            #[cfg(all(unix, feature = "remote_desktop_portal", not(target_os = "macos")))]
            Backend::Xdp => write!(f, "xdg-desktop-portal"),
            #[cfg(all(unix, feature = "x11", not(target_os = "macos")))]
            Backend::X11 => write!(f, "X11"),
            #[cfg(windows)]
            Backend::Windows => write!(f, "windows"),
            #[cfg(target_os = "macos")]
            Backend::MacOs => write!(f, "macos"),
            #[cfg(all(target_os = "macos", feature = "macos_vhid"))]
            Backend::MacOsVHID => write!(f, "macos-vhid"),
            Backend::Dummy => write!(f, "dummy"),
        }
    }
}

pub struct InputEmulation {
    emulation: Box<dyn Emulation>,
    handles: HashSet<EmulationHandle>,
    pressed_keys: HashMap<EmulationHandle, HashSet<u32>>,
    mouse_config: MouseConfig,
}

impl InputEmulation {
    async fn with_backend(backend: Backend) -> Result<InputEmulation, EmulationCreationError> {
        let emulation: Box<dyn Emulation> = match backend {
            #[cfg(all(unix, feature = "wlroots", not(target_os = "macos")))]
            Backend::Wlroots => Box::new(wlroots::WlrootsEmulation::new()?),
            #[cfg(all(unix, feature = "libei", not(target_os = "macos")))]
            Backend::Libei => Box::new(libei::LibeiEmulation::new().await?),
            #[cfg(all(unix, feature = "x11", not(target_os = "macos")))]
            Backend::X11 => Box::new(x11::X11Emulation::new()?),
            #[cfg(all(unix, feature = "remote_desktop_portal", not(target_os = "macos")))]
            Backend::Xdp => Box::new(xdg_desktop_portal::DesktopPortalEmulation::new().await?),
            #[cfg(windows)]
            Backend::Windows => Box::new(windows::WindowsEmulation::new()?),
            #[cfg(target_os = "macos")]
            Backend::MacOs => Box::new(macos::MacOSEmulation::new()?),
            #[cfg(all(target_os = "macos", feature = "macos_vhid"))]
            Backend::MacOsVHID => Box::new(macos_vhid::VirtualHIDEmulation::new()?),
            Backend::Dummy => Box::new(dummy::DummyEmulation::new()),
        };
        Ok(Self {
            emulation,
            handles: HashSet::new(),
            pressed_keys: HashMap::new(),
            mouse_config: MouseConfig::default(),
        })
    }

    pub fn set_mouse_config(&mut self, config: MouseConfig) {
        log::info!(
            "mouse config: speed={:.2}, scroll_speed={:.2}, natural_scrolling={}",
            config.speed,
            config.scroll_speed,
            config.natural_scrolling
        );
        self.mouse_config = config;
    }

    fn apply_mouse_config(&self, event: Event) -> Event {
        match event {
            Event::Pointer(PointerEvent::Motion { time, dx, dy }) => {
                Event::Pointer(PointerEvent::Motion {
                    time,
                    dx: dx * self.mouse_config.speed,
                    dy: dy * self.mouse_config.speed,
                })
            }
            Event::Pointer(PointerEvent::Axis { time, axis, value }) => {
                let dir = if self.mouse_config.natural_scrolling {
                    -1.0
                } else {
                    1.0
                };
                Event::Pointer(PointerEvent::Axis {
                    time,
                    axis,
                    value: value * self.mouse_config.scroll_speed * dir,
                })
            }
            Event::Pointer(PointerEvent::AxisDiscrete120 { axis, value }) => {
                let dir = if self.mouse_config.natural_scrolling {
                    -1
                } else {
                    1
                };
                // For discrete scroll, round-to-nearest instead of truncating
                // to avoid collapsing small values to zero at sub-1.0 speeds.
                let scaled_f = value as f64 * self.mouse_config.scroll_speed;
                let scaled = if scaled_f.abs() < 1.0 && scaled_f != 0.0 {
                    scaled_f.signum() as i32 // preserve at least 1 unit
                } else {
                    scaled_f.round() as i32
                };
                Event::Pointer(PointerEvent::AxisDiscrete120 {
                    axis,
                    value: scaled * dir,
                })
            }
            other => other,
        }
    }

    pub async fn new(backend: Option<Backend>) -> Result<InputEmulation, EmulationCreationError> {
        if let Some(backend) = backend {
            let b = Self::with_backend(backend).await;
            if b.is_ok() {
                log::info!("using emulation backend: {backend}");
            }
            return b;
        }

        for backend in [
            #[cfg(all(unix, feature = "wlroots", not(target_os = "macos")))]
            Backend::Wlroots,
            #[cfg(all(unix, feature = "libei", not(target_os = "macos")))]
            Backend::Libei,
            #[cfg(all(unix, feature = "remote_desktop_portal", not(target_os = "macos")))]
            Backend::Xdp,
            #[cfg(all(unix, feature = "x11", not(target_os = "macos")))]
            Backend::X11,
            #[cfg(windows)]
            Backend::Windows,
            // prefer VirtualHID over standard CGEventPost when available
            #[cfg(all(target_os = "macos", feature = "macos_vhid"))]
            Backend::MacOsVHID,
            #[cfg(target_os = "macos")]
            Backend::MacOs,
            Backend::Dummy,
        ] {
            match Self::with_backend(backend).await {
                Ok(b) => {
                    log::info!("using emulation backend: {backend}");
                    return Ok(b);
                }
                Err(e) if e.cancelled_by_user() => return Err(e),
                Err(e) => log::warn!("{e}"),
            }
        }

        Err(EmulationCreationError::NoAvailableBackend)
    }

    pub async fn consume(
        &mut self,
        event: Event,
        handle: EmulationHandle,
    ) -> Result<(), EmulationError> {
        // apply mouse speed and scroll normalization
        let event = self.apply_mouse_config(event);

        match event {
            Event::Keyboard(KeyboardEvent::Key { key, state, .. }) => {
                // prevent double pressed / released keys
                if self.update_pressed_keys(handle, key, state) {
                    self.emulation.consume(event, handle).await?;
                }
                Ok(())
            }
            _ => self.emulation.consume(event, handle).await,
        }
    }

    pub async fn create(&mut self, handle: EmulationHandle) -> bool {
        if self.handles.insert(handle) {
            self.pressed_keys.insert(handle, HashSet::new());
            self.emulation.create(handle).await;
            true
        } else {
            false
        }
    }

    pub async fn destroy(&mut self, handle: EmulationHandle) {
        let _ = self.release_keys(handle).await;
        if self.handles.remove(&handle) {
            self.pressed_keys.remove(&handle);
            self.emulation.destroy(handle).await
        }
    }

    pub async fn terminate(&mut self) {
        for handle in self.handles.iter().cloned().collect::<Vec<_>>() {
            self.destroy(handle).await
        }
        self.emulation.terminate().await
    }

    pub async fn release_keys(&mut self, handle: EmulationHandle) -> Result<(), EmulationError> {
        if let Some(keys) = self.pressed_keys.get_mut(&handle) {
            let keys = keys.drain().collect::<Vec<_>>();
            for key in keys {
                let event = Event::Keyboard(KeyboardEvent::Key {
                    time: 0,
                    key,
                    state: 0,
                });
                self.emulation.consume(event, handle).await?;
                if let Ok(key) = input_event::scancode::Linux::try_from(key) {
                    log::warn!("releasing stuck key: {key:?}");
                }
            }
        }

        let event = Event::Keyboard(KeyboardEvent::Modifiers {
            depressed: 0,
            latched: 0,
            locked: 0,
            group: 0,
        });
        self.emulation.consume(event, handle).await?;
        Ok(())
    }

    pub fn has_pressed_keys(&self, handle: EmulationHandle) -> bool {
        self.pressed_keys
            .get(&handle)
            .is_some_and(|p| !p.is_empty())
    }

    /// update the pressed_keys for the given handle
    /// returns whether the event should be processed
    fn update_pressed_keys(&mut self, handle: EmulationHandle, key: u32, state: u8) -> bool {
        let Some(pressed_keys) = self.pressed_keys.get_mut(&handle) else {
            return false;
        };

        if state == 0 {
            // currently pressed => can release
            pressed_keys.remove(&key)
        } else {
            // currently not pressed => can press
            pressed_keys.insert(key)
        }
    }
}

#[async_trait]
trait Emulation: Send {
    async fn consume(
        &mut self,
        event: Event,
        handle: EmulationHandle,
    ) -> Result<(), EmulationError>;
    async fn create(&mut self, handle: EmulationHandle);
    async fn destroy(&mut self, handle: EmulationHandle);
    async fn terminate(&mut self);
}
