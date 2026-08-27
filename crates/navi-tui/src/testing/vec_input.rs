//! A test-friendly [`InputSource`] that yields a queued sequence of events.

use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use crossterm::event::Event;

use crate::InputSource;

/// A scripted input source for the TUI event loop. Events are pushed with
/// [`Self::push`] and consumed in order by `read()`. `poll()` always
/// reports "ready" until the queue is empty, then reports "not ready" so
/// the loop's idle draw ticks can still fire.
pub struct VecInput {
    events: VecDeque<Event>,
    stop_when_empty: bool,
}

impl VecInput {
    pub fn new() -> Self {
        Self {
            events: VecDeque::new(),
            stop_when_empty: false,
        }
    }

    /// When `true`, `poll` returns `Ok(false)` once the queue is empty so
    /// the harness can detect end-of-script.
    pub fn stop_when_empty(mut self, value: bool) -> Self {
        self.stop_when_empty = value;
        self
    }

    pub fn push(&mut self, event: Event) {
        self.events.push_back(event);
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }
}

impl Default for VecInput {
    fn default() -> Self {
        Self::new()
    }
}

impl InputSource for VecInput {
    fn poll(&mut self, _timeout: Duration) -> io::Result<bool> {
        if self.events.is_empty() {
            Ok(!self.stop_when_empty)
        } else {
            Ok(true)
        }
    }

    fn read(&mut self) -> io::Result<Event> {
        self.events
            .pop_front()
            .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "VecInput: no more events"))
    }
}
