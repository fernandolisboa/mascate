//! Asking a Platform again, a little later each time, when it says to wait
//! or fails for a moment.

use std::time::Duration;

use mascate_kernel::PlatformError;

/// Waits between attempts. Injected, so tests never sleep.
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

/// How long to wait before the first retry, doubling before each next one,
/// and how many retries to make.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Backoff {
    pub first: Duration,
    pub retries: u32,
}

/// The outcome of one attempt.
pub(crate) enum Attempt<T> {
    /// Final, whether it worked or not.
    Done(Result<T, PlatformError>),
    /// Worth trying again after a pause.
    Again(PlatformError),
}

/// Runs `attempt` until it is done or the retries run out; the last error
/// is the answer then.
pub(crate) fn retrying<T>(
    pause: &dyn Pause,
    backoff: Backoff,
    mut attempt: impl FnMut() -> Attempt<T>,
) -> Result<T, PlatformError> {
    let mut wait = backoff.first;
    let mut retries = 0;
    loop {
        match attempt() {
            Attempt::Done(answer) => return answer,
            Attempt::Again(error) if retries == backoff.retries => return Err(error),
            Attempt::Again(_) => {
                pause.pause(wait);
                wait *= 2;
                retries += 1;
            }
        }
    }
}
