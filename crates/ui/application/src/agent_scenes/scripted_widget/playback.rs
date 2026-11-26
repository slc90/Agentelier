use super::widget::{Playback, Timer};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Clock {
    interval: Option<Duration>,
    last: Option<Instant>,
    elapsed: Duration,
}

impl Clock {
    pub fn running(&self) -> bool {
        self.interval.is_some()
    }

    pub fn request(&mut self, description: Playback) {
        if description.reset_clock {
            self.elapsed = Duration::ZERO;
            self.last = None;
        }
        self.interval = description
            .running
            .then(|| Duration::from_millis(description.interval_ms));
        if !self.running() {
            self.last = None;
        }
    }

    pub fn timer(&mut self) -> Option<Timer> {
        let interval = self.interval?;
        let last = *self.last.get_or_insert_with(Instant::now);
        Some(Timer {
            due: last + interval,
            last,
            elapsed: self.elapsed,
        })
    }

    pub fn ticked(&mut self, at: Instant, elapsed: Duration) {
        self.last = Some(at);
        self.elapsed = elapsed;
    }

    pub fn suspend(&mut self) {
        self.last = None;
    }

    pub fn stop(&mut self) {
        self.interval = None;
        self.last = None;
    }
}
