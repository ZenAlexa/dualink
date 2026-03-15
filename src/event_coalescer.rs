use input_event::{Event, PointerEvent};
use std::time::{Duration, Instant};

/// Coalesces high-frequency mouse motion events within a configurable
/// time window to reduce network traffic from high-poll-rate mice (>1000Hz).
///
/// All non-motion events (buttons, keys, scroll) pass through immediately,
/// flushing any accumulated motion first to preserve ordering.
pub struct EventCoalescer {
    window: Duration,
    acc_dx: f64,
    acc_dy: f64,
    deadline: Option<Instant>,
}

impl EventCoalescer {
    /// Create a new coalescer.  `window` of zero disables coalescing.
    pub fn new(window: Duration) -> Self {
        Self {
            window,
            acc_dx: 0.0,
            acc_dy: 0.0,
            deadline: None,
        }
    }

    /// Returns true if coalescing is disabled (window is zero).
    pub fn is_disabled(&self) -> bool {
        self.window.is_zero()
    }

    /// Feed an event into the coalescer.
    ///
    /// Returns `(flushed_motion, passthrough)`:
    /// - For motion events: accumulates deltas, returns `(None, None)`.
    /// - For non-motion events: flushes accumulated motion first,
    ///   then returns the original event as passthrough.
    pub fn feed(&mut self, event: Event) -> (Option<Event>, Option<Event>) {
        if self.is_disabled() {
            return (None, Some(event));
        }

        match event {
            Event::Pointer(PointerEvent::Motion { time: _, dx, dy }) => {
                self.acc_dx += dx;
                self.acc_dy += dy;
                if self.deadline.is_none() {
                    self.deadline = Some(Instant::now() + self.window);
                }
                (None, None)
            }
            other => {
                // Non-motion event: flush accumulated motion first
                let flushed = self.take_accumulated();
                (flushed, Some(other))
            }
        }
    }

    /// Flush accumulated motion if any.  Call when the deadline expires.
    pub fn flush(&mut self) -> Option<Event> {
        self.take_accumulated()
    }

    /// Instant at which accumulated motion should be flushed.
    /// Returns `None` when there is nothing buffered.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// True when there is buffered motion waiting to be flushed.
    pub fn has_pending(&self) -> bool {
        self.deadline.is_some()
    }

    fn take_accumulated(&mut self) -> Option<Event> {
        if self.acc_dx == 0.0 && self.acc_dy == 0.0 {
            self.deadline = None;
            return None;
        }
        let event = Event::Pointer(PointerEvent::Motion {
            time: 0,
            dx: self.acc_dx,
            dy: self.acc_dy,
        });
        self.acc_dx = 0.0;
        self.acc_dy = 0.0;
        self.deadline = None;
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use input_event::{BTN_LEFT, KeyboardEvent};

    #[test]
    fn disabled_coalescer_passes_through() {
        let mut c = EventCoalescer::new(Duration::ZERO);
        let ev = Event::Pointer(PointerEvent::Motion {
            time: 0,
            dx: 1.0,
            dy: 2.0,
        });
        let (flushed, pass) = c.feed(ev);
        assert!(flushed.is_none());
        assert_eq!(pass, Some(ev));
    }

    #[test]
    fn motion_events_are_accumulated() {
        let mut c = EventCoalescer::new(Duration::from_millis(1));
        let m1 = Event::Pointer(PointerEvent::Motion {
            time: 0,
            dx: 1.0,
            dy: 2.0,
        });
        let m2 = Event::Pointer(PointerEvent::Motion {
            time: 0,
            dx: 3.0,
            dy: -1.0,
        });

        assert_eq!(c.feed(m1), (None, None));
        assert!(c.has_pending());
        assert_eq!(c.feed(m2), (None, None));

        let flushed = c.flush().unwrap();
        assert_eq!(
            flushed,
            Event::Pointer(PointerEvent::Motion {
                time: 0,
                dx: 4.0,
                dy: 1.0,
            })
        );
        assert!(!c.has_pending());
    }

    #[test]
    fn non_motion_flushes_accumulated_motion() {
        let mut c = EventCoalescer::new(Duration::from_millis(1));

        let motion = Event::Pointer(PointerEvent::Motion {
            time: 0,
            dx: 5.0,
            dy: 5.0,
        });
        c.feed(motion);

        let button = Event::Pointer(PointerEvent::Button {
            time: 0,
            button: BTN_LEFT,
            state: 1,
        });
        let (flushed, pass) = c.feed(button);

        assert_eq!(
            flushed,
            Some(Event::Pointer(PointerEvent::Motion {
                time: 0,
                dx: 5.0,
                dy: 5.0,
            }))
        );
        assert_eq!(pass, Some(button));
    }

    #[test]
    fn keyboard_events_pass_through() {
        let mut c = EventCoalescer::new(Duration::from_millis(1));
        let key = Event::Keyboard(KeyboardEvent::Key {
            time: 0,
            key: 42,
            state: 1,
        });
        let (flushed, pass) = c.feed(key);
        assert!(flushed.is_none());
        assert_eq!(pass, Some(key));
    }

    #[test]
    fn flush_with_nothing_returns_none() {
        let mut c = EventCoalescer::new(Duration::from_millis(1));
        assert!(c.flush().is_none());
    }

    #[test]
    fn deadline_is_set_on_first_motion() {
        let mut c = EventCoalescer::new(Duration::from_millis(1));
        assert!(c.next_deadline().is_none());

        let motion = Event::Pointer(PointerEvent::Motion {
            time: 0,
            dx: 1.0,
            dy: 0.0,
        });
        c.feed(motion);
        assert!(c.next_deadline().is_some());
    }
}
