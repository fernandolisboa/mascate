//! Waiting a moment between calls to a Platform.

use std::time::Duration;

/// Waits between attempts or between posts. Injected, so tests never sleep.
pub trait Pause: Send + Sync {
    fn pause(&self, duration: Duration);
}

/// Blocks the calling thread, which is never the UI's.
pub struct ThreadPause;

impl Pause for ThreadPause {
    fn pause(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}
